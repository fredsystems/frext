//! Filesystem operations for the sidebar context menus: rename, create file,
//! create directory, and move-to-trash.
//!
//! These are factored out of the UI loop so the validation and the disk
//! effects can be unit-tested without an egui context. Every operation returns
//! a typed [`FsError`]; the UI layer logs failures (and surfaces them to the
//! user) rather than panicking.

use std::path::{Path, PathBuf};

use crate::error::FsError;

/// Validate that `name` is usable as a single file or directory name: it must
/// be non-empty, must not be `.` or `..`, and must not contain a path
/// separator (so it cannot escape its parent directory).
fn validate_name(name: &str) -> Result<(), FsError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(FsError::InvalidName(name.to_owned()));
    }
    Ok(())
}

/// The parent directory of `path`, or [`FsError::NoParent`] when it has none.
fn parent_of(path: &Path) -> Result<&Path, FsError> {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| FsError::NoParent(path.to_path_buf()))
}

/// Rename the file or directory at `path` to `new_name` within the same
/// parent directory. Returns the new full path on success.
///
/// Refuses to overwrite an existing entry, and rejects a `new_name` that is
/// empty or contains a path separator. Renaming to the current name is a
/// no-op that returns the original path.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, FsError> {
    validate_name(new_name)?;
    let parent = parent_of(path)?;
    let dest = parent.join(new_name);

    if dest == path {
        return Ok(dest);
    }
    if dest.exists() {
        return Err(FsError::AlreadyExists(dest));
    }

    std::fs::rename(path, &dest).map_err(|source| FsError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(dest)
}

/// Create an empty file named `name` inside the directory `dir`. Returns the
/// new file's path.
///
/// Refuses to overwrite an existing entry and rejects an invalid `name`.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf, FsError> {
    validate_name(name)?;
    let dest = dir.join(name);

    if dest.exists() {
        return Err(FsError::AlreadyExists(dest));
    }

    // `create_new` fails if the file already appeared between the check above
    // and now, closing the small race without clobbering.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
        .map_err(|source| FsError::Io {
            path: dest.clone(),
            source,
        })?;
    Ok(dest)
}

/// Create a directory named `name` inside the directory `dir`. Returns the new
/// directory's path.
///
/// Refuses to overwrite an existing entry and rejects an invalid `name`.
pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf, FsError> {
    validate_name(name)?;
    let dest = dir.join(name);

    if dest.exists() {
        return Err(FsError::AlreadyExists(dest));
    }

    std::fs::create_dir(&dest).map_err(|source| FsError::Io {
        path: dest.clone(),
        source,
    })?;
    Ok(dest)
}

/// Move the file or directory at `path` to the operating system's trash, so
/// the deletion is recoverable.
pub fn move_to_trash(path: &Path) -> Result<(), FsError> {
    trash::delete(path).map_err(|err| FsError::Trash {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}

#[cfg(test)]
mod tests {
    // `unwrap` is acceptable in test code: a panic on an unexpected `Err`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// An isolated temp directory unique to a test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frext-fsops-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn validate_name_rejects_empty_dots_and_separators() {
        assert!(validate_name("").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\\b").is_err());
        assert!(validate_name("ok.txt").is_ok());
        // A leading-dot dotfile is a legitimate single name.
        assert!(validate_name(".gitignore").is_ok());
    }

    #[test]
    fn rename_moves_within_parent_and_returns_new_path() {
        let dir = temp_dir("rename");
        let src = dir.join("old.txt");
        std::fs::write(&src, "body").unwrap();

        let dest = rename(&src, "new.txt").unwrap();

        assert_eq!(dest, dir.join("new.txt"));
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "body");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_refuses_to_clobber_existing() {
        let dir = temp_dir("rename-clobber");
        let src = dir.join("a.txt");
        let other = dir.join("b.txt");
        std::fs::write(&src, "a").unwrap();
        std::fs::write(&other, "b").unwrap();

        let err = rename(&src, "b.txt").unwrap_err();
        assert!(matches!(err, FsError::AlreadyExists(_)));
        // Both files are untouched.
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "b");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_to_same_name_is_a_noop() {
        let dir = temp_dir("rename-same");
        let src = dir.join("same.txt");
        std::fs::write(&src, "x").unwrap();

        let dest = rename(&src, "same.txt").unwrap();
        assert_eq!(dest, src);
        assert!(src.exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_rejects_invalid_name() {
        let dir = temp_dir("rename-invalid");
        let src = dir.join("a.txt");
        std::fs::write(&src, "x").unwrap();

        assert!(matches!(
            rename(&src, "../escape").unwrap_err(),
            FsError::InvalidName(_)
        ));
        assert!(src.exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn create_file_makes_an_empty_file() {
        let dir = temp_dir("create-file");

        let path = create_file(&dir, "fresh.txt").unwrap();
        assert!(path.is_file());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn create_file_refuses_existing() {
        let dir = temp_dir("create-file-exists");
        std::fs::write(dir.join("there.txt"), "x").unwrap();

        let err = create_file(&dir, "there.txt").unwrap_err();
        assert!(matches!(err, FsError::AlreadyExists(_)));
        // The pre-existing file's contents are untouched.
        assert_eq!(std::fs::read_to_string(dir.join("there.txt")).unwrap(), "x");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn create_dir_makes_a_directory() {
        let dir = temp_dir("create-dir");

        let path = create_dir(&dir, "sub").unwrap();
        assert!(path.is_dir());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn create_dir_refuses_existing() {
        let dir = temp_dir("create-dir-exists");
        std::fs::create_dir(dir.join("sub")).unwrap();

        let err = create_dir(&dir, "sub").unwrap_err();
        assert!(matches!(err, FsError::AlreadyExists(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
