//! Links and shims on disk.
//!
//! `<home>/links/<name>` points at a content directory: a relative symlink on
//! Unix, a directory junction on Windows (junctions need no privilege).
//! `<home>/bin/<name>` launches a file through the link, so shims never change
//! when the link is repointed. Removing a link never touches its target.
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use panned::{Error, ErrorCode};

use crate::store::io_error;

/// Link names: letters, digits, `-`, `_` and `.`, not starting with `.`.
pub(crate) fn validate_name(name: &str) -> Result<(), Error> {
    let valid = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            format!("invalid link name {name:?}: use letters, digits, - _ ."),
        )
        .with_detail("name", name))
    }
}

/// A relative path inside content: no absolute parts, no `..`.
pub(crate) fn validate_file(file: &str) -> Result<PathBuf, Error> {
    let path = Path::new(file);
    if file.is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(
            Error::new(ErrorCode::InvalidRequest, format!("unsafe file path {file:?}"))
                .with_detail("file", file),
        );
    }
    Ok(path.to_path_buf())
}

#[cfg(windows)]
fn absolute(path: &Path) -> Result<PathBuf, Error> {
    std::path::absolute(path).map_err(|e| io_error(path, e))
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The path of link `name`.
pub(crate) fn link_path(home: &Path, name: &str) -> PathBuf {
    home.join("links").join(name)
}

/// Point link `name` at `target`, replacing any previous target.
pub(crate) fn point(home: &Path, name: &str, target: &Path) -> Result<PathBuf, Error> {
    let dir = home.join("links");
    fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
    let link = dir.join(name);
    let staged = dir.join(format!(".{name}.{}", uuid::Uuid::new_v4()));
    #[cfg(unix)]
    {
        let relative = Path::new("..").join(target.strip_prefix(home).unwrap_or(target));
        std::os::unix::fs::symlink(&relative, &staged).map_err(|e| io_error(&staged, e))?;
        // rename replaces the old symlink atomically.
        fs::rename(&staged, &link).map_err(|e| io_error(&link, e))?;
    }
    #[cfg(windows)]
    {
        junction::create(absolute(target)?, &staged).map_err(|e| io_error(&staged, e))?;
        // Windows cannot rename over a directory: remove the old junction, then move the new one in.
        if exists(&link) {
            fs::remove_dir(&link).map_err(|e| io_error(&link, e))?;
        }
        fs::rename(&staged, &link).map_err(|e| io_error(&link, e))?;
    }
    Ok(link)
}

/// Write the launcher `<home>/bin/<name>` for `file` inside link `name`.
pub(crate) fn write_shim(home: &Path, name: &str, file: &Path) -> Result<PathBuf, Error> {
    let bin = home.join("bin");
    fs::create_dir_all(&bin).map_err(|e| io_error(&bin, e))?;
    let shim = bin.join(name);
    let through_link = Path::new("..").join("links").join(name).join(file);
    #[cfg(unix)]
    {
        let staged = bin.join(format!(".{name}.{}", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&through_link, &staged).map_err(|e| io_error(&staged, e))?;
        fs::rename(&staged, &shim).map_err(|e| io_error(&shim, e))?;
        Ok(shim)
    }
    #[cfg(windows)]
    {
        let windows_path = through_link.to_string_lossy().replace('/', "\\");
        let cmd = bin.join(format!("{name}.cmd"));
        fs::write(&cmd, format!("@\"%~dp0{windows_path}\" %*\r\n")).map_err(|e| io_error(&cmd, e))?;
        let posix_path = through_link.to_string_lossy().replace('\\', "/");
        let script = format!("#!/bin/sh\nexec \"$(dirname \"$0\")/{posix_path}\" \"$@\"\n");
        fs::write(&shim, script).map_err(|e| io_error(&shim, e))?;
        Ok(cmd)
    }
}

/// Remove link `name` and its shims, if present.
pub(crate) fn remove(home: &Path, name: &str) -> Result<(), Error> {
    let link = link_path(home, name);
    if exists(&link) {
        // A symlink is a file on Unix; a junction is a directory on Windows.
        let removed = if cfg!(windows) {
            fs::remove_dir(&link)
        } else {
            fs::remove_file(&link)
        };
        removed.map_err(|e| io_error(&link, e))?;
    }
    remove_shims(home, name)
}

/// Remove the launchers of link `name`, if present.
pub(crate) fn remove_shims(home: &Path, name: &str) -> Result<(), Error> {
    let bin = home.join("bin");
    for shim in [bin.join(name), bin.join(format!("{name}.cmd"))] {
        if exists(&shim) {
            fs::remove_file(&shim).map_err(|e| io_error(&shim, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_files_are_validated() {
        for good in ["milkpan", "gh-cli", "tool_2.1"] {
            assert!(validate_name(good).is_ok(), "{good}");
        }
        for bad in ["", ".hidden", "a/b", "a b", "..", "x\\y"] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        assert!(validate_file("bin/tool").is_ok());
        assert!(validate_file("../tool").is_err());
        assert!(validate_file("/etc/passwd").is_err());
    }

    #[test]
    fn links_repoint_without_touching_targets() {
        let home = tempfile::tempdir().unwrap();
        let (a, b) = (
            home.path().join("trees").join("a"),
            home.path().join("trees").join("b"),
        );
        for (dir, text) in [(&a, "one"), (&b, "two")] {
            fs::create_dir_all(dir).unwrap();
            fs::write(dir.join("version"), text).unwrap();
        }
        let link = point(home.path(), "tool", &a).unwrap();
        assert_eq!(fs::read_to_string(link.join("version")).unwrap(), "one");
        point(home.path(), "tool", &b).unwrap();
        assert_eq!(fs::read_to_string(link.join("version")).unwrap(), "two");
        remove(home.path(), "tool").unwrap();
        assert!(!exists(&link));
        assert!(
            a.join("version").is_file() && b.join("version").is_file(),
            "targets are untouched"
        );
    }
}
