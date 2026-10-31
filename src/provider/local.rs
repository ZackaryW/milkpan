//! The built-in local provider: snapshots of local git repositories at exact
//! commits, through the `git` command. Only committed content is used.
use std::{
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
};

use panned::{Error, ErrorCode};

use crate::{
    code,
    provider::{Provider, Resolved},
    spec::Spec,
    store::ContentKind,
};

/// The local provider, for prefix `local`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Local;

/// Validate a local spec and put its path in canonical form: absolute, `/`
/// separators, no trailing `/`. The shared grammar splits a Windows drive
/// (`C:/x`) into source `C` and path `/x`; they are joined again here.
pub fn canonicalize(mut spec: Spec) -> Result<Spec, Error> {
    let original = spec.canonical();
    let invalid = |why: &str| {
        Error::new(
            ErrorCode::InvalidRequest,
            format!("invalid local spec {original}: {why}"),
        )
        .with_detail("spec", original.as_str())
    };
    if spec.asset.is_some() {
        return Err(invalid("local specs take no #asset"));
    }
    let mut path = spec.source.clone();
    if let Some(inner) = spec.path.take() {
        if path.len() != 1 || !path.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(invalid("local specs take no inner path"));
        }
        path = format!("{path}:/{}", inner.trim_start_matches(['/', '\\']));
    }
    let path = path.replace('\\', "/");
    let path = if path.len() > 1 {
        path.trim_end_matches('/').to_string()
    } else {
        path
    };
    let absolute = path.starts_with('/')
        || (path.len() >= 3 && path.as_bytes()[1] == b':' && path.as_bytes()[2] == b'/');
    if !absolute {
        return Err(invalid("the repository path must be absolute"));
    }
    spec.source = path;
    Ok(spec)
}

fn git(spec: &Spec) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(Path::new(&spec.source))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    command
}

fn unavailable(spec: &Spec, error: io::Error) -> Error {
    Error::new(
        ErrorCode::Unavailable,
        format!("cannot run git for {spec}: {error}"),
    )
    .with_detail("spec", spec.canonical())
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

impl Provider for Local {
    fn prefixes(&self) -> Vec<String> {
        vec!["local".into()]
    }

    fn canonicalize(&self, spec: Spec) -> Result<Spec, Error> {
        canonicalize(spec)
    }

    fn kind(&self, _: &Spec) -> String {
        "snapshot".into()
    }

    fn resolve(&self, spec: &Spec) -> Result<Resolved, Error> {
        let reference = spec.selector.as_deref().unwrap_or("HEAD");
        let output = git(spec)
            .args(["rev-parse", "--verify", "--end-of-options"])
            .arg(format!("{reference}^{{commit}}"))
            .output()
            .map_err(|e| unavailable(spec, e))?;
        if !output.status.success() {
            return Err(Error::new(
                ErrorCode::NotFound,
                format!("{spec} does not resolve to a commit"),
            )
            .with_detail("spec", spec.canonical())
            .with_detail("git", first_line(&output.stderr)));
        }
        let version = first_line(&output.stdout);
        Ok(Resolved {
            version,
            kind: ContentKind::Archive,
        })
    }

    fn fetch(&self, spec: &Spec, resolved: &Resolved, out: &mut dyn Write) -> Result<(), Error> {
        let mut child = git(spec)
            .args(["archive", "--format=tar.gz", "--prefix=snapshot/"])
            .arg(&resolved.version)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| unavailable(spec, e))?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let copied = io::copy(&mut stdout, out);
        let output = child.wait_with_output().map_err(|e| unavailable(spec, e))?;
        if !output.status.success() || copied.is_err() {
            return Err(
                Error::new(code("fetch_failed"), format!("git archive failed for {spec}"))
                    .with_detail("spec", spec.canonical())
                    .with_detail("git", first_line(&output.stderr)),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::store::Store;

    fn canonical(text: &str) -> Result<String, Error> {
        canonicalize(Spec::parse(text)?).map(|s| s.canonical())
    }

    #[test]
    fn paths_are_canonical_and_absolute() {
        assert_eq!(
            canonical(r"local:C:\work\tools@main").unwrap(),
            "local:C:/work/tools@main"
        );
        assert_eq!(canonical("local:/srv/tools/").unwrap(), "local:/srv/tools");
        let again = canonical("local:C:/work/tools@main").unwrap();
        assert_eq!(canonical(&again).unwrap(), again, "canonical form is stable");
        for bad in ["local:tools@main", "local:/srv/tools#asset", "local:ab:/x"] {
            assert_eq!(
                canonical(bad).unwrap_err().code,
                ErrorCode::InvalidRequest,
                "{bad}"
            );
        }
    }

    fn sh(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        sh(dir.path(), &["init", "-q", "-b", "main"]);
        sh(dir.path(), &["config", "user.email", "test@example.invalid"]);
        sh(dir.path(), &["config", "user.name", "test"]);
        fs::write(dir.path().join("README.md"), "v1").unwrap();
        sh(dir.path(), &["add", "."]);
        sh(dir.path(), &["commit", "-q", "-m", "one"]);
        dir
    }

    fn spec_for(dir: &Path, selector: &str) -> Spec {
        let path = dir.to_string_lossy().replace('\\', "/");
        canonicalize(Spec::parse(&format!("local:{path}{selector}")).unwrap()).unwrap()
    }

    #[test]
    fn snapshots_hold_committed_content_only() {
        let repo = repository();
        fs::write(repo.path().join("README.md"), "uncommitted").unwrap();
        fs::write(repo.path().join("untracked.txt"), "x").unwrap();
        let spec = spec_for(repo.path(), "");
        let resolved = Local.resolve(&spec).unwrap();
        assert_eq!(resolved.version.len(), 40);
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path());
        let digest = store.ingest(|out| Local.fetch(&spec, &resolved, out)).unwrap();
        let tree = store.tree(&digest).unwrap();
        assert_eq!(fs::read_to_string(tree.join("README.md")).unwrap(), "v1");
        assert!(!tree.join("untracked.txt").exists() && !tree.join(".git").exists());
    }

    #[test]
    fn branches_move_and_unknown_refs_are_not_found() {
        let repo = repository();
        let main = spec_for(repo.path(), "@main");
        let first = Local.resolve(&main).unwrap().version;
        fs::write(repo.path().join("README.md"), "v2").unwrap();
        sh(repo.path(), &["commit", "-q", "-am", "two"]);
        assert_ne!(Local.resolve(&main).unwrap().version, first);
        let missing = Local
            .resolve(&spec_for(repo.path(), "@no-such-branch"))
            .unwrap_err();
        assert_eq!(missing.code, ErrorCode::NotFound);
        let nowhere = spec_for(&repo.path().join("missing"), "");
        assert_eq!(Local.resolve(&nowhere).unwrap_err().code, ErrorCode::NotFound);
    }
}
