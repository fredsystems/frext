//! Command-line argument parsing.

use std::ffi::OsString;
use std::path::PathBuf;

/// Paths given on the command line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Files to open as tabs, in command-line order.
    pub files: Vec<PathBuf>,
    /// The workspace root for the sidebar: the first directory argument.
    pub dir: Option<PathBuf>,
}

/// Parse the arguments after the program name.
///
/// The first directory becomes [`Args::dir`]; later directories are
/// ignored. Everything else is treated as a file to open.
#[must_use]
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Args {
    let mut parsed = Args::default();
    for arg in args {
        let path = PathBuf::from(arg);
        if path.is_dir() {
            if parsed.dir.is_none() {
                parsed.dir = Some(path);
            }
        } else {
            parsed.files.push(path);
        }
    }
    parsed
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
            "frext-term-cli-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn os(paths: &[&PathBuf]) -> Vec<OsString> {
        paths.iter().map(|p| p.as_os_str().to_owned()).collect()
    }

    #[test]
    fn no_args_is_empty() {
        assert_eq!(parse(Vec::new()), Args::default());
    }

    #[test]
    fn files_keep_order() {
        let root = temp_dir("order");
        let b = root.join("b.txt");
        let a = root.join("a.txt");
        let missing = root.join("missing.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let args = parse(os(&[&b, &missing, &a]));

        assert_eq!(args.files, vec![b, missing, a]);
        assert_eq!(args.dir, None);
    }

    #[test]
    fn first_dir_wins() {
        let root = temp_dir("first-dir");
        let file = root.join("f.txt");
        let dir = root.join("ws");
        fs::write(&file, "f").unwrap();
        fs::create_dir_all(&dir).unwrap();

        let args = parse(os(&[&file, &dir]));

        assert_eq!(args.files, vec![file]);
        assert_eq!(args.dir, Some(dir));
    }

    #[test]
    fn second_dir_is_ignored() {
        let root = temp_dir("second-dir");
        let first = root.join("one");
        let second = root.join("two");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        let args = parse(os(&[&first, &second]));

        assert_eq!(args.dir, Some(first));
        assert!(args.files.is_empty());
    }
}
