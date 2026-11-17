//! Workspace maintenance tasks, run as `cargo xtask <command>`.
//!
//! `nerd-icons <devicons-dir> [<out>]` generates `frext-term`'s Nerd Font
//! icon table from an `nvim-tree/nvim-web-devicons` checkout.

mod nerd_icons;

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// Usage text printed on a bad command line.
const USAGE: &str = "usage: cargo xtask nerd-icons <devicons-dir> [<out>]";

/// The generated table, relative to the workspace root.
const DEFAULT_OUT: &str = "frext-term/src/icons/nerd_table.rs";

/// Data files inside a devicons checkout.
const DATA_DIR: &str = "lua/nvim-web-devicons/default";

/// A parsed command line.
#[derive(Debug, PartialEq, Eq)]
enum Task {
    /// Generate the Nerd icon table.
    NerdIcons {
        /// The `nvim-web-devicons` checkout.
        devicons: PathBuf,
        /// Where to write the table.
        out: PathBuf,
    },
}

/// Everything that can go wrong in an xtask.
#[derive(Debug, thiserror::Error)]
enum XtaskError {
    /// The command line is malformed.
    #[error("{0}")]
    Usage(String),
    /// A devicons data file could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// `git` could not be run.
    #[error("cannot run git: {0}")]
    GitSpawn(io::Error),
    /// `git rev-parse HEAD` failed or printed no commit.
    #[error("git rev-parse HEAD failed in {0}")]
    GitFailed(PathBuf),
    /// A devicons data file could not be parsed.
    #[error("cannot parse {path}: {source}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: nerd_icons::NerdIconsError,
    },
    /// The table could not be generated.
    #[error("cannot generate the table: {0}")]
    Generate(#[from] nerd_icons::NerdIconsError),
    /// The table could not be written.
    #[error("cannot write {path}: {source}")]
    Write {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse_args(&args, &workspace_root()).and_then(|task| run(&task));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(XtaskError::Usage(message)) => {
            eprintln!("error: {message}\n{USAGE}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The workspace root: the parent of this crate's manifest directory.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map_or_else(|| manifest_dir.to_path_buf(), Path::to_path_buf)
}

/// Parse the arguments after the program name.
fn parse_args(args: &[String], workspace_root: &Path) -> Result<Task, XtaskError> {
    match args {
        [] => Err(XtaskError::Usage("missing command".to_owned())),
        [command, rest @ ..] if command == "nerd-icons" => match rest {
            [] => Err(XtaskError::Usage("missing <devicons-dir>".to_owned())),
            [devicons] => Ok(Task::NerdIcons {
                devicons: PathBuf::from(devicons),
                out: workspace_root.join(DEFAULT_OUT),
            }),
            [devicons, out] => Ok(Task::NerdIcons {
                devicons: PathBuf::from(devicons),
                out: PathBuf::from(out),
            }),
            _ => Err(XtaskError::Usage("too many arguments".to_owned())),
        },
        [command, ..] => Err(XtaskError::Usage(format!("unknown command `{command}`"))),
    }
}

/// Run a parsed task.
fn run(task: &Task) -> Result<(), XtaskError> {
    match task {
        Task::NerdIcons { devicons, out } => nerd_icons_task(devicons, out),
    }
}

/// Generate the Nerd icon table from `devicons` into `out`.
fn nerd_icons_task(devicons: &Path, out: &Path) -> Result<(), XtaskError> {
    let data = devicons.join(DATA_DIR);
    let extensions = parse(&data.join("icons_by_file_extension.lua"))?;
    let filenames = parse(&data.join("icons_by_filename.lua"))?;
    let commit = head_commit(devicons)?;

    let votes = nerd_icons::votes(&extensions, &filenames);
    let table = nerd_icons::render_table(
        &commit,
        &nerd_icons::entries(&votes, &extensions, &filenames)?,
    );

    let write_error = |source| XtaskError::Write {
        path: out.to_path_buf(),
        source,
    };
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(write_error)?;
    }
    fs::write(out, table).map_err(write_error)?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Read and parse a devicons data file.
fn parse(path: &Path) -> Result<Vec<nerd_icons::DevIcon>, XtaskError> {
    nerd_icons::parse_devicons(&read(path)?).map_err(|source| XtaskError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Read a file to a string.
fn read(path: &Path) -> Result<String, XtaskError> {
    fs::read_to_string(path).map_err(|source| XtaskError::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// The `HEAD` commit of the git checkout at `dir`.
fn head_commit(dir: &Path) -> Result<String, XtaskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(XtaskError::GitSpawn)?;
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() && !commit.is_empty() {
        Ok(commit)
    } else {
        Err(XtaskError::GitFailed(dir.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected error is
    // exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|&item| item.to_owned()).collect()
    }

    fn is_usage(result: Result<Task, XtaskError>) -> bool {
        matches!(result, Err(XtaskError::Usage(_)))
    }

    #[test]
    fn missing_command_is_a_usage_error() {
        assert!(is_usage(parse_args(&[], Path::new("/ws"))));
    }

    #[test]
    fn missing_dir_is_a_usage_error() {
        assert!(is_usage(parse_args(
            &args(&["nerd-icons"]),
            Path::new("/ws")
        )));
    }

    #[test]
    fn unknown_command_is_a_usage_error() {
        assert!(is_usage(parse_args(
            &args(&["frobnicate", "x"]),
            Path::new("/ws")
        )));
    }

    #[test]
    fn too_many_arguments_is_a_usage_error() {
        let result = parse_args(&args(&["nerd-icons", "a", "b", "c"]), Path::new("/ws"));
        assert!(is_usage(result));
    }

    #[test]
    fn out_defaults_under_the_workspace_root() {
        let task = parse_args(&args(&["nerd-icons", "/dev/icons"]), Path::new("/ws"));
        assert_eq!(
            task.ok(),
            Some(Task::NerdIcons {
                devicons: PathBuf::from("/dev/icons"),
                out: PathBuf::from("/ws/frext-term/src/icons/nerd_table.rs"),
            })
        );
    }

    #[test]
    fn explicit_out_is_used() {
        let task = parse_args(
            &args(&["nerd-icons", "/dev/icons", "o.rs"]),
            Path::new("/ws"),
        );
        assert_eq!(
            task.ok(),
            Some(Task::NerdIcons {
                devicons: PathBuf::from("/dev/icons"),
                out: PathBuf::from("o.rs"),
            })
        );
    }

    #[test]
    fn a_data_file_without_entries_is_a_parse_error() {
        let dir = std::env::temp_dir().join(format!("xtask-empty-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.lua");
        fs::write(&path, "return {\n}\n").unwrap();
        let result = parse(&path);
        fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(
            result,
            Err(XtaskError::Parse {
                source: nerd_icons::NerdIconsError::NoEntries,
                ..
            })
        ));
    }

    #[test]
    fn workspace_root_contains_this_crate() {
        assert!(workspace_root().join("xtask/Cargo.toml").is_file());
    }
}
