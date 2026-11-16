//! Tab completion for the path prompts.
//!
//! [`resolve`] turns prompt text into a path, [`candidates`] lists the
//! directory entries that complete it, and [`Completion`] holds the
//! state of repeated Tab presses (common-prefix extension, then cycling
//! through the candidates).

use std::path::{Path, PathBuf};

use frext_core::editor::StepDirection;

/// Resolves prompt text to a path.
///
/// `~` alone expands to `home` and `~/…` joins onto it; when `home` is
/// `None` the text is treated as a literal relative path. Absolute paths
/// are returned unchanged, and relative paths (including the empty
/// string) join onto `base`. The result is not canonicalized.
#[must_use]
pub fn resolve(input: &str, base: &Path, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if input == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = input.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    if input.is_empty() {
        return base.to_path_buf();
    }
    let path = Path::new(input);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Lists the completions of `input`.
///
/// The input splits at its last `/` into a dir part and a name prefix.
/// The dir part is resolved with [`resolve`] (an empty dir part lists
/// `base`) and read; a read error gives an empty list. Entries match by
/// a case-sensitive prefix, and dotfiles are included only when the
/// prefix starts with `.`. Each candidate is the dir part followed by
/// the entry name, plus a trailing `/` for directories (symlinks are
/// followed). Directories come first, then files, each sorted by name.
#[must_use]
pub fn candidates(input: &str, base: &Path, home: Option<&Path>) -> Vec<String> {
    let (dir_part, prefix) = match input.rfind('/') {
        Some(i) => input.split_at(i + 1),
        None => ("", input),
    };
    let dir = if dir_part.is_empty() {
        base.to_path_buf()
    } else {
        resolve(dir_part, base, home)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let show_hidden = prefix.starts_with('.');
    let mut found: Vec<(bool, String)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with(prefix) || (name.starts_with('.') && !show_hidden) {
                return None;
            }
            let is_dir = std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir());
            Some((is_dir, name))
        })
        .collect();
    found.sort_by(|(a_dir, a), (b_dir, b)| b_dir.cmp(a_dir).then_with(|| a.cmp(b)));
    found
        .into_iter()
        .map(|(is_dir, name)| {
            let mut candidate = format!("{dir_part}{name}");
            if is_dir {
                candidate.push('/');
            }
            candidate
        })
        .collect()
}

/// The longest prefix shared by every item, cut on a char boundary.
/// An empty slice gives an empty string.
#[must_use]
pub fn longest_common_prefix(items: &[String]) -> String {
    let Some((first, rest)) = items.split_first() else {
        return String::new();
    };
    let mut len = first.len();
    for item in rest {
        len = first
            .char_indices()
            .zip(item.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, a), _)| i + a.len_utf8())
            .min(len);
    }
    first[..len].to_string()
}

/// What a prompt should draw under its field: the candidate list and the
/// selected entry. An empty list means draw nothing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CompletionView {
    /// The candidates being cycled through.
    pub candidates: Vec<String>,
    /// The index of the candidate currently in the field.
    pub selected: Option<usize>,
}

/// The Tab-completion state of a path prompt.
#[derive(Debug, Default)]
pub struct Completion {
    cycle: Option<(Vec<String>, usize)>,
}

impl Completion {
    /// Handles a Tab (or Shift+Tab for [`StepDirection::Backward`]) press,
    /// updating `text` in place.
    ///
    /// On a first press: no candidates changes nothing; one candidate
    /// replaces the text; several replace the text with their longest
    /// common prefix when that is longer, else start cycling at the
    /// first (or, backward, last) candidate. While cycling, each press
    /// steps through the stored list with wrapping. If `text` no longer
    /// matches the selected candidate, the press counts as a first one.
    pub fn tab(
        &mut self,
        text: &mut String,
        base: &Path,
        home: Option<&Path>,
        direction: StepDirection,
    ) -> CompletionView {
        if let Some((list, index)) = &mut self.cycle {
            if list.get(*index) == Some(text) {
                let len = list.len();
                *index = match direction {
                    StepDirection::Forward => (*index + 1) % len,
                    StepDirection::Backward => (*index + len - 1) % len,
                };
                text.clone_from(&list[*index]);
                return self.view();
            }
            self.cycle = None;
        }

        let found = candidates(text, base, home);
        match found.as_slice() {
            [] => {}
            [only] => text.clone_from(only),
            _ => {
                let common = longest_common_prefix(&found);
                if common.len() > text.len() {
                    *text = common;
                } else {
                    let index = match direction {
                        StepDirection::Forward => 0,
                        StepDirection::Backward => found.len() - 1,
                    };
                    text.clone_from(&found[index]);
                    self.cycle = Some((found, index));
                }
            }
        }
        self.view()
    }

    /// Stops cycling; the prompt calls this on any edit.
    pub fn reset(&mut self) {
        self.cycle = None;
    }

    /// Whether repeated Tabs are stepping through a candidate list.
    #[must_use]
    pub fn is_cycling(&self) -> bool {
        self.cycle.is_some()
    }

    /// The current candidate list and selection, empty when not cycling.
    #[must_use]
    pub fn view(&self) -> CompletionView {
        match &self.cycle {
            Some((list, index)) => CompletionView {
                candidates: list.clone(),
                selected: Some(*index),
            },
            None => CompletionView::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-term-complete-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A temp dir holding `src/`, `src/main.rs`, `Cargo.toml`,
    /// `Cargo.lock` and `.hidden`.
    fn fixture(tag: &str) -> PathBuf {
        let root = temp_dir(tag);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("Cargo.lock"), "").unwrap();
        fs::write(root.join(".hidden"), "").unwrap();
        root
    }

    #[test]
    fn tilde_expands_to_home() {
        let home = Path::new("/home/me");
        let base = Path::new("/work");
        assert_eq!(resolve("~", base, Some(home)), PathBuf::from("/home/me"));
        assert_eq!(
            resolve("~/notes.txt", base, Some(home)),
            PathBuf::from("/home/me/notes.txt")
        );
        assert_eq!(resolve("~/x", base, None), PathBuf::from("/work/~/x"));
    }

    #[test]
    fn relative_and_absolute_resolution() {
        let base = Path::new("/work");
        assert_eq!(resolve("a/b.rs", base, None), PathBuf::from("/work/a/b.rs"));
        assert_eq!(
            resolve("/etc/hosts", base, None),
            PathBuf::from("/etc/hosts")
        );
        assert_eq!(resolve("", base, None), PathBuf::from("/work"));
    }

    #[test]
    fn directories_sort_first() {
        let root = fixture("dir-first");
        assert_eq!(
            candidates("", &root, None),
            vec!["src/", "Cargo.lock", "Cargo.toml"]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dotfiles_only_with_dot_prefix() {
        let root = fixture("dotfiles");
        assert!(!candidates("", &root, None).contains(&".hidden".to_string()));
        assert_eq!(candidates(".", &root, None), vec![".hidden"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn candidates_keep_the_dir_part() {
        let root = fixture("dir-part");
        assert_eq!(candidates("src/m", &root, None), vec!["src/main.rs"]);
        let home = root.clone();
        assert_eq!(candidates("~/sr", &root, Some(&home)), vec!["~/src/"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn single_match_completes_directory() {
        let root = fixture("single");
        let mut completion = Completion::default();
        let mut text = String::from("src");
        let view = completion.tab(&mut text, &root, None, StepDirection::Forward);
        assert_eq!(text, "src/");
        assert_eq!(view, CompletionView::default());
        assert!(!completion.is_cycling());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_match_changes_nothing() {
        let root = fixture("no-match");
        let mut completion = Completion::default();
        let mut text = String::from("zzz");
        let view = completion.tab(&mut text, &root, None, StepDirection::Forward);
        assert_eq!(text, "zzz");
        assert_eq!(view, CompletionView::default());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn common_prefix_extends_text() {
        let root = fixture("common");
        let mut completion = Completion::default();
        let mut text = String::from("Ca");
        let view = completion.tab(&mut text, &root, None, StepDirection::Forward);
        assert_eq!(text, "Cargo.");
        assert_eq!(view, CompletionView::default());
        assert!(!completion.is_cycling());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn second_tab_cycles_and_backward_wraps() {
        let root = fixture("cycle");
        let mut completion = Completion::default();
        let mut text = String::from("Ca");
        completion.tab(&mut text, &root, None, StepDirection::Forward);

        let view = completion.tab(&mut text, &root, None, StepDirection::Forward);
        assert_eq!(text, "Cargo.lock");
        assert_eq!(view.candidates, vec!["Cargo.lock", "Cargo.toml"]);
        assert_eq!(view.selected, Some(0));
        assert!(completion.is_cycling());
        assert_eq!(completion.view(), view);

        let view = completion.tab(&mut text, &root, None, StepDirection::Backward);
        assert_eq!(text, "Cargo.toml");
        assert_eq!(view.selected, Some(1));

        let view = completion.tab(&mut text, &root, None, StepDirection::Forward);
        assert_eq!(text, "Cargo.lock");
        assert_eq!(view.selected, Some(0));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn backward_first_cycle_starts_at_last() {
        let root = fixture("backward-start");
        let mut completion = Completion::default();
        let mut text = String::from("Cargo.");
        let view = completion.tab(&mut text, &root, None, StepDirection::Backward);
        assert_eq!(text, "Cargo.toml");
        assert_eq!(view.selected, Some(1));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reset_stops_cycling() {
        let root = fixture("reset");
        let mut completion = Completion::default();
        let mut text = String::from("Cargo.");
        completion.tab(&mut text, &root, None, StepDirection::Forward);
        assert!(completion.is_cycling());
        completion.reset();
        assert!(!completion.is_cycling());
        assert_eq!(completion.view(), CompletionView::default());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn external_edit_restarts_completion() {
        let root = fixture("external-edit");
        let mut completion = Completion::default();
        let mut text = String::from("Cargo.");
        completion.tab(&mut text, &root, None, StepDirection::Forward);
        assert!(completion.is_cycling());
        text = String::from("sr");
        let view = completion.tab(&mut text, &root, None, StepDirection::Forward);
        assert_eq!(text, "src/");
        assert!(!completion.is_cycling());
        assert_eq!(view, CompletionView::default());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unreadable_dir_gives_no_candidates() {
        let root = fixture("unreadable");
        assert!(candidates("missing/", &root, None).is_empty());
        assert!(candidates("Cargo.toml/", &root, None).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn common_prefix_respects_char_boundaries() {
        let items = vec!["héllo".to_string(), "hé€".to_string(), "héllo!".to_string()];
        assert_eq!(longest_common_prefix(&items), "hé");
        let items = vec!["é".to_string(), "è".to_string()];
        assert_eq!(longest_common_prefix(&items), "");
        assert_eq!(longest_common_prefix(&[]), "");
        assert_eq!(longest_common_prefix(&["日本".to_string()]), "日本");
    }
}
