//! Immutable content-addressed blobs and the trees extracted from archives.
//!
//! Content is written into `tmp/` while hashing and renamed into place, so a
//! blob path either holds complete content or does not exist.
use std::{
    fs::{self, File},
    io::{self, Write},
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
    /// The blob is a gzip tar archive presented as a directory tree.
    Archive,
}

pub(crate) fn io_error(path: &Path, error: io::Error) -> Error {
    Error::new(ErrorCode::Internal, format!("{}: {error}", path.display()))
        .with_detail("path", path.display().to_string())
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

    /// The tree extracted from a gzip tar blob, extracting it on first use.
    /// The archive's top-level directory is stripped; links and special files
    /// are skipped; absolute paths and `..` components are rejected.
    pub fn tree(&self, digest: &str) -> Result<PathBuf, Error> {
        let target = self.tree_path(digest);
        if target.is_dir() {
            return Ok(target);
        }
        let staging = self.dir("tmp")?.join(uuid::Uuid::new_v4().to_string());
        let extracted = self.extract(digest, &staging);
        if let Err(error) = extracted {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        self.dir("trees")?;
        if fs::rename(&staging, &target).is_err() {
            // Another process extracted the same blob first; trees are identical.
            let _ = fs::remove_dir_all(&staging);
            if !target.is_dir() {
                return Err(Error::new(
                    ErrorCode::Internal,
                    format!("cannot place tree {}", target.display()),
                ));
            }
        }
        Ok(target)
    }

    fn extract(&self, digest: &str, staging: &Path) -> Result<(), Error> {
        let blob = self.blob_path(digest);
        let file = File::open(&blob).map_err(|e| io_error(&blob, e))?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
        fs::create_dir_all(staging).map_err(|e| io_error(staging, e))?;
        let bad = |message: String| {
            Error::new(
                ErrorCode::tool("milkpan", "fetch_failed").expect("valid code"),
                message,
            )
            .with_detail("digest", digest)
        };
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
                relative_entry(&path).ok_or_else(|| bad(format!("unsafe entry path {}", path.display())))?;
            let Some(relative) = relative else { continue };
            let out = staging.join(&relative);
            if kind.is_dir() {
                fs::create_dir_all(&out).map_err(|e| io_error(&out, e))?;
                continue;
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            }
            let mut file =
                File::create(&out).map_err(|_| bad(format!("cannot write entry {}", relative.display())))?;
            io::copy(&mut entry, &mut file).map_err(|e| io_error(&out, e))?;
        }
        Ok(())
    }

    /// Delete a blob and its tree, if present.
    pub fn remove(&self, digest: &str) -> Result<(), Error> {
        let tree = self.tree_path(digest);
        if tree.exists() {
            fs::remove_dir_all(&tree).map_err(|e| io_error(&tree, e))?;
        }
        let blob = self.blob_path(digest);
        match fs::remove_file(&blob) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_error(&blob, e)),
            _ => Ok(()),
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

    /// Remove staging files left by interrupted operations.
    pub fn clear_tmp(&self) -> Result<(), Error> {
        let tmp = self.root.join("tmp");
        if tmp.exists() {
            fs::remove_dir_all(&tmp).map_err(|e| io_error(&tmp, e))?;
        }
        Ok(())
    }
}

/// The entry path without the archive's top-level directory. `Some(None)` for
/// the top-level directory itself, `None` for unsafe paths.
fn relative_entry(path: &Path) -> Option<Option<PathBuf>> {
    let mut components = path.components();
    match components.next()? {
        Component::Normal(_) => {}
        _ => return None,
    }
    let mut relative = PathBuf::new();
    for component in components {
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
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(store.blob_path(digest))
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[cfg(test)]
pub(crate) fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, path, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
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
    fn tarballs_extract_without_their_top_level_directory() {
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
        store.remove(&digest).unwrap();
        assert!(!tree.exists() && !store.has(&digest));
    }

    #[test]
    fn unsafe_archive_paths_are_rejected() {
        assert_eq!(
            relative_entry(Path::new("top/a/b")),
            Some(Some(PathBuf::from("a/b")))
        );
        assert_eq!(relative_entry(Path::new("top/")), Some(None));
        assert_eq!(relative_entry(Path::new("top/../escape")), None);
        assert_eq!(relative_entry(Path::new("/etc/passwd")), None);
        assert_eq!(relative_entry(Path::new("../x")), None);
    }
}
