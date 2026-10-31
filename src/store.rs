//! Immutable content-addressed blobs, the trees extracted from archives and
//! named views of single-file blobs.
//!
//! Content is written into `tmp/` while hashing and renamed into place, so a
//! blob path either holds complete content or does not exist.
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use panned::{Error, ErrorCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How stored content is presented: as the blob itself, or as a tree extracted from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    /// The blob is the content (a single file or a release asset).
    File,
    /// The blob is a gzip tar or zip archive presented as a directory tree.
    Archive,
}

/// What deleting stored content did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// The blob and everything derived from it are gone.
    Removed,
    /// The platform refused because something is in use; nothing was deleted.
    InUse,
}

pub(crate) fn io_error(path: &Path, error: io::Error) -> Error {
    Error::new(ErrorCode::Internal, format!("{}: {error}", path.display()))
        .with_detail("path", path.display().to_string())
}

/// Whether the platform refused an operation because a file is open or running.
fn in_use(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(5 | 32) if cfg!(windows))
}

/// The blob and tree store under a bucket home.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// A store rooted at `root` (the bucket home).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dir(&self, name: &str) -> Result<PathBuf, Error> {
        let dir = self.root.join(name);
        fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
        Ok(dir)
    }

    /// Where the blob with `digest` lives, whether or not it exists.
    pub fn blob_path(&self, digest: &str) -> PathBuf {
        self.root
            .join("blobs")
            .join("sha256")
            .join(&digest[..2])
            .join(&digest[2..])
    }

    /// Where the tree extracted from `digest` lives, whether or not it exists.
    pub fn tree_path(&self, digest: &str) -> PathBuf {
        self.root.join("trees").join(digest)
    }

    /// Where named views of the single-file blob `digest` live.
    pub fn view_path(&self, digest: &str) -> PathBuf {
        self.root.join("files").join(digest)
    }

    /// Whether a blob is stored.
    pub fn has(&self, digest: &str) -> bool {
        self.blob_path(digest).is_file()
    }

    /// Stream content from `fill` into the store and return its SHA-256 digest.
    /// Content already stored is not written twice.
    pub fn ingest(&self, fill: impl FnOnce(&mut dyn Write) -> Result<(), Error>) -> Result<String, Error> {
        let tmp = self.dir("tmp")?.join(uuid::Uuid::new_v4().to_string());
        let file = File::create(&tmp).map_err(|e| io_error(&tmp, e))?;
        let mut writer = HashingWriter {
            file,
            hasher: Sha256::new(),
        };
        let filled = fill(&mut writer).and_then(|()| writer.file.sync_all().map_err(|e| io_error(&tmp, e)));
        if let Err(error) = filled {
            let _ = fs::remove_file(&tmp);
            return Err(error);
        }
        let digest = hex(&writer.hasher.finalize());
        let target = self.blob_path(&digest);
        if target.is_file() {
            let _ = fs::remove_file(&tmp);
        } else {
            let parent = target.parent().expect("blob paths have a parent");
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            fs::rename(&tmp, &target).map_err(|e| io_error(&target, e))?;
        }
        Ok(digest)
    }

    /// Re-hash a stored blob and compare it with its address.
    pub fn verify(&self, digest: &str) -> Result<bool, Error> {
        let path = self.blob_path(digest);
        let mut file = File::open(&path).map_err(|e| io_error(&path, e))?;
        let mut hasher = Sha256::new();
        io::copy(&mut file, &mut hasher).map_err(|e| io_error(&path, e))?;
        Ok(hex(&hasher.finalize()) == digest)
    }

    /// The tree extracted from a gzip tar or zip blob, extracting it on first
    /// use. A top-level directory shared by every entry is stripped; links and
    /// special files are skipped; absolute paths and `..` components are
    /// rejected; executable bits are kept on Unix.
    pub fn tree(&self, digest: &str) -> Result<PathBuf, Error> {
        let target = self.tree_path(digest);
        if target.is_dir() {
            return Ok(target);
        }
        let staging = self.dir("tmp")?.join(uuid::Uuid::new_v4().to_string());
        let raw = staging.join("raw");
        let placed = self.extract(digest, &raw).and_then(|()| {
            let root = single_directory(&raw)?.unwrap_or(raw.clone());
            self.dir("trees")?;
            if fs::rename(&root, &target).is_err() && !target.is_dir() {
                return Err(Error::new(
                    ErrorCode::Internal,
                    format!("cannot place tree {}", target.display()),
                ));
            }
            // Otherwise another process extracted the same blob first; trees are identical.
            Ok(())
        });
        let _ = fs::remove_dir_all(&staging);
        placed.map(|()| target)
    }

    fn extract(&self, digest: &str, out: &Path) -> Result<(), Error> {
        let blob = self.blob_path(digest);
        let mut file = File::open(&blob).map_err(|e| io_error(&blob, e))?;
        let mut magic = [0u8; 4];
        let read = file.read(&mut magic).map_err(|e| io_error(&blob, e))?;
        drop(file);
        fs::create_dir_all(out).map_err(|e| io_error(out, e))?;
        let file = File::open(&blob).map_err(|e| io_error(&blob, e))?;
        let bad =
            |message: String| Error::new(crate::code("fetch_failed"), message).with_detail("digest", digest);
        match &magic[..read] {
            [0x1f, 0x8b, ..] => extract_tar(file, out, &bad),
            [0x50, 0x4b, 0x03, 0x04] => extract_zip(file, out, &bad),
            _ => Err(bad("content is not a gzip tar or zip archive".into())),
        }
    }

    /// A directory holding the single-file blob `digest` under `name`, created on
    /// first use as a hard link (or a copy when linking fails).
    pub fn file_view(&self, digest: &str, name: &str, executable: bool) -> Result<PathBuf, Error> {
        if Path::new(name).components().count() != 1
            || !matches!(Path::new(name).components().next(), Some(Component::Normal(_)))
        {
            return Err(
                Error::new(ErrorCode::InvalidRequest, format!("invalid file name {name:?}"))
                    .with_detail("file", name),
            );
        }
        let dir = self.view_path(digest);
        let path = dir.join(name);
        if !path.is_file() {
            fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
            let blob = self.blob_path(digest);
            if fs::hard_link(&blob, &path).is_err() {
                fs::copy(&blob, &path).map_err(|e| io_error(&path, e))?;
            }
        }
        if executable {
            set_mode(&path, Some(0o755))?;
        }
        Ok(dir)
    }

    /// Delete a blob and everything derived from it. When the platform refuses
    /// because something is in use (a running program on Windows), nothing is
    /// deleted and [`Removal::InUse`] is returned.
    pub fn remove(&self, digest: &str) -> Result<Removal, Error> {
        // Move derived directories aside first: on Windows the move fails while a
        // file inside is open, before anything has been deleted.
        for dir in [self.view_path(digest), self.tree_path(digest)] {
            if !dir.exists() {
                continue;
            }
            let trash = self.dir("tmp")?.join(format!("trash-{}", uuid::Uuid::new_v4()));
            match fs::rename(&dir, &trash) {
                Ok(()) => {
                    // Leftovers are cleared later as stale staging entries.
                    let _ = fs::remove_dir_all(&trash);
                }
                Err(e) if in_use(&e) => return Ok(Removal::InUse),
                Err(e) => return Err(io_error(&dir, e)),
            }
        }
        let blob = self.blob_path(digest);
        match fs::remove_file(&blob) {
            Ok(()) => Ok(Removal::Removed),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Removal::Removed),
            Err(e) if in_use(&e) => Ok(Removal::InUse),
            Err(e) => Err(io_error(&blob, e)),
        }
    }

    /// Every stored blob digest.
    pub fn digests(&self) -> Result<Vec<String>, Error> {
        let root = self.root.join("blobs").join("sha256");
        let mut digests = Vec::new();
        let Ok(prefixes) = fs::read_dir(&root) else {
            return Ok(digests);
        };
        for prefix in prefixes.flatten() {
            let head = prefix.file_name().to_string_lossy().into_owned();
            for blob in fs::read_dir(prefix.path())
                .map_err(|e| io_error(&prefix.path(), e))?
                .flatten()
            {
                digests.push(format!("{head}{}", blob.file_name().to_string_lossy()));
            }
        }
        digests.sort();
        Ok(digests)
    }

    /// Remove staging entries older than `age`, left by interrupted operations.
    /// Younger entries may belong to a download still running in another
    /// process, so they are kept. Returns how many entries were removed.
    pub fn clear_stale_tmp(&self, age: std::time::Duration) -> Result<usize, Error> {
        let tmp = self.root.join("tmp");
        let Ok(entries) = fs::read_dir(&tmp) else {
            return Ok(0);
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|e| e >= age);
            if !stale {
                continue;
            }
            let path = entry.path();
            let result = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            result.map_err(|e| io_error(&path, e))?;
            removed += 1;
        }
        Ok(removed)
    }
}

fn extract_tar(file: File, out: &Path, bad: &dyn Fn(String) -> Error) -> Result<(), Error> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in archive
        .entries()
        .map_err(|e| bad(format!("unreadable archive: {e}")))?
    {
        let mut entry = entry.map_err(|e| bad(format!("unreadable archive entry: {e}")))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| bad(format!("unreadable entry path: {e}")))?
            .into_owned();
        let relative =
            safe_relative(&path).ok_or_else(|| bad(format!("unsafe entry path {}", path.display())))?;
        let Some(relative) = relative else { continue };
        let target = out.join(&relative);
        if kind.is_dir() {
            fs::create_dir_all(&target).map_err(|e| io_error(&target, e))?;
            continue;
        }
        let mode = entry.header().mode().ok();
        write_entry(&target, &relative, &mut entry, mode, bad)?;
    }
    Ok(())
}

fn extract_zip(file: File, out: &Path, bad: &dyn Fn(String) -> Error) -> Result<(), Error> {
    let mut archive = zip::ZipArchive::new(file).map_err(|e| bad(format!("unreadable archive: {e}")))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| bad(format!("unreadable archive entry: {e}")))?;
        if entry.is_symlink() {
            continue;
        }
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| bad(format!("unsafe entry path {}", entry.name())))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = out.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| io_error(&target, e))?;
            continue;
        }
        let mode = entry.unix_mode();
        write_entry(&target, &relative, &mut entry, mode, bad)?;
    }
    Ok(())
}

fn write_entry(
    target: &Path,
    relative: &Path,
    content: &mut dyn Read,
    mode: Option<u32>,
    bad: &dyn Fn(String) -> Error,
) -> Result<(), Error> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    let mut file =
        File::create(target).map_err(|_| bad(format!("cannot write entry {}", relative.display())))?;
    io::copy(content, &mut file).map_err(|e| io_error(target, e))?;
    drop(file);
    set_mode(target, mode)
}

/// Keep only whether a file is executable: `0o755` or `0o644`. No-op off Unix.
fn set_mode(path: &Path, mode: Option<u32>) -> Result<(), Error> {
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        let mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| io_error(path, e))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// The sole entry of `dir` when it is a directory, so a shared top level can be stripped.
fn single_directory(dir: &Path) -> Result<Option<PathBuf>, Error> {
    let mut entries = fs::read_dir(dir).map_err(|e| io_error(dir, e))?.flatten();
    match (entries.next(), entries.next()) {
        (Some(only), None) if only.path().is_dir() => Ok(Some(only.path())),
        _ => Ok(None),
    }
}

/// The entry path when safe. `Some(None)` for an empty path, `None` for
/// absolute paths and `..` components.
fn safe_relative(path: &Path) -> Option<Option<PathBuf>> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some((!relative.as_os_str().is_empty()).then_some(relative))
}

struct HashingWriter {
    file: File,
    hasher: Sha256,
}

impl Write for HashingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.file.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read a stored blob fully; used by tests and small files.
#[cfg(test)]
pub(crate) fn read_blob(store: &Store, digest: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    File::open(store.blob_path(digest))
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

/// A gzip tarball of `(path, content, mode)` entries.
#[cfg(test)]
pub(crate) fn tarball_with_modes(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, data, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, path, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[cfg(test)]
pub(crate) fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let entries: Vec<_> = entries.iter().map(|(p, d)| (*p, *d, 0o644)).collect();
    tarball_with_modes(&entries)
}

/// A zip archive of `(path, content)` entries.
#[cfg(test)]
pub(crate) fn zipball(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    for (path, data) in entries {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(store: &Store, bytes: &[u8]) -> String {
        store
            .ingest(|w| w.write_all(bytes).map_err(|e| io_error(Path::new("test"), e)))
            .unwrap()
    }

    #[test]
    fn identical_content_is_stored_once() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let a = put(&store, b"hello");
        let b = put(&store, b"hello");
        assert_eq!(a, b);
        assert_eq!(
            a,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(store.digests().unwrap(), std::slice::from_ref(&a));
        assert_eq!(read_blob(&store, &a), b"hello");
        assert!(store.verify(&a).unwrap());
        assert_eq!(
            fs::read_dir(home.path().join("tmp")).unwrap().count(),
            0,
            "staging is cleaned"
        );
    }

    #[test]
    fn only_stale_staging_entries_are_cleared() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let tmp = home.path().join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        let old = File::create(tmp.join("old")).unwrap();
        let two_days = std::time::Duration::from_secs(2 * 86_400);
        old.set_modified(std::time::SystemTime::now() - two_days).unwrap();
        File::create(tmp.join("running")).unwrap();
        assert_eq!(
            store
                .clear_stale_tmp(std::time::Duration::from_secs(86_400))
                .unwrap(),
            1
        );
        assert!(!tmp.join("old").exists() && tmp.join("running").exists());
    }

    #[test]
    fn failed_ingest_leaves_nothing() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let error = store.ingest(|w| {
            w.write_all(b"partial").unwrap();
            Err(Error::new(ErrorCode::Unavailable, "network dropped"))
        });
        assert!(error.is_err());
        assert!(store.digests().unwrap().is_empty());
        assert_eq!(fs::read_dir(home.path().join("tmp")).unwrap().count(), 0);
    }

    #[test]
    fn tarballs_extract_without_their_shared_top_level_directory() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let digest = put(
            &store,
            &tarball(&[
                ("octo-tools-abc/README.md", b"hi"),
                ("octo-tools-abc/docs/a.md", b"a"),
            ]),
        );
        let tree = store.tree(&digest).unwrap();
        assert_eq!(fs::read_to_string(tree.join("README.md")).unwrap(), "hi");
        assert_eq!(fs::read_to_string(tree.join("docs/a.md")).unwrap(), "a");
        assert_eq!(store.tree(&digest).unwrap(), tree, "extracted once");
        assert_eq!(store.remove(&digest).unwrap(), Removal::Removed);
        assert!(!tree.exists() && !store.has(&digest));
    }

    #[test]
    fn zips_extract_and_root_level_archives_stay_whole() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let zipped = put(
            &store,
            &zipball(&[("tool-1.0/tool.exe", b"binary"), ("tool-1.0/README.md", b"r")]),
        );
        assert_eq!(
            fs::read(store.tree(&zipped).unwrap().join("tool.exe")).unwrap(),
            b"binary"
        );
        let flat = put(&store, &tarball(&[("tool", b"binary"), ("LICENSE", b"mit")]));
        let tree = store.tree(&flat).unwrap();
        assert!(tree.join("tool").is_file() && tree.join("LICENSE").is_file());
        let plain = put(&store, b"not an archive");
        assert_eq!(
            store.tree(&plain).unwrap_err().code.as_str(),
            "milkpan.fetch_failed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_entries_stay_executable() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let digest = put(
            &store,
            &tarball_with_modes(&[("top/tool", b"#!/bin/sh\n", 0o755), ("top/doc", b"d", 0o600)]),
        );
        let tree = store.tree(&digest).unwrap();
        assert_eq!(
            fs::metadata(tree.join("tool")).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(tree.join("doc")).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn file_views_name_single_file_blobs() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let digest = put(&store, b"#!/bin/sh\necho hi\n");
        let dir = store.file_view(&digest, "tool", true).unwrap();
        assert_eq!(fs::read(dir.join("tool")).unwrap(), b"#!/bin/sh\necho hi\n");
        assert!(store.file_view(&digest, "../x", false).is_err());
        assert_eq!(store.remove(&digest).unwrap(), Removal::Removed);
        assert!(!dir.exists());
    }

    #[cfg(windows)]
    #[test]
    fn content_in_use_is_left_untouched() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let digest = put(
            &store,
            &zipball(&[("top/tool.exe", b"binary"), ("top/other", b"o")]),
        );
        let tree = store.tree(&digest).unwrap();
        let open = File::open(tree.join("tool.exe")).unwrap();
        assert_eq!(store.remove(&digest).unwrap(), Removal::InUse);
        assert!(
            tree.join("other").is_file() && store.has(&digest),
            "nothing was deleted"
        );
        drop(open);
        assert_eq!(store.remove(&digest).unwrap(), Removal::Removed);
    }

    #[test]
    fn unsafe_archive_paths_are_rejected() {
        assert_eq!(
            safe_relative(Path::new("top/a/b")),
            Some(Some(PathBuf::from("top/a/b")))
        );
        assert_eq!(safe_relative(Path::new("./")), Some(None));
        assert_eq!(safe_relative(Path::new("top/../escape")), None);
        assert_eq!(safe_relative(Path::new("/etc/passwd")), None);
        assert_eq!(safe_relative(Path::new("../x")), None);
    }
}
