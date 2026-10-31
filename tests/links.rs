//! Links follow their lease, shims launch through links, linked leases stay
//! held, and content still in use survives reclamation.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use milkpan::{Bucket, BucketBuilder, Clock, ContentKind, Provider, Resolved, Spec};
use panned::{Error, ErrorCode, Principal, Scope};

/// A released tool: `rel:<name>` resolves to the current version, whose
/// archive holds a script printing that version and exiting with 3.
#[derive(Clone, Default)]
struct Releases(Arc<Mutex<String>>);

impl Releases {
    fn publish(&self, version: &str) {
        *self.0.lock().unwrap() = version.into();
    }
}

#[cfg(windows)]
const TOOL: &str = "tool.cmd";
#[cfg(not(windows))]
const TOOL: &str = "tool";

fn script(version: &str) -> String {
    if cfg!(windows) {
        format!("@echo {version} %*\r\n@exit /b 3\r\n")
    } else {
        format!("#!/bin/sh\necho {version} \"$@\"\nexit 3\n")
    }
}

fn archive(version: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let body = script(version);
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    builder
        .append_data(&mut header, format!("tool-{version}/{TOOL}"), body.as_bytes())
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

impl Provider for Releases {
    fn prefixes(&self) -> Vec<String> {
        vec!["rel".into(), "raw".into()]
    }
    fn kind(&self, _: &Spec) -> String {
        "release".into()
    }
    fn resolve(&self, spec: &Spec) -> Result<Resolved, Error> {
        let kind = if spec.prefix == "raw" {
            ContentKind::File
        } else {
            ContentKind::Archive
        };
        Ok(Resolved {
            version: self.0.lock().unwrap().clone(),
            kind,
        })
    }
    fn fetch(&self, spec: &Spec, resolved: &Resolved, out: &mut dyn Write) -> Result<(), Error> {
        let bytes = if spec.prefix == "raw" {
            script(&resolved.version).into_bytes()
        } else {
            archive(&resolved.version)
        };
        out.write_all(&bytes)
            .map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))
    }
}

#[derive(Clone, Default)]
struct TestClock(Arc<AtomicU64>);

impl Clock for TestClock {
    fn now(&self) -> u64 {
        1_000_000 + self.0.load(Ordering::SeqCst)
    }
}

struct Setup {
    home: tempfile::TempDir,
    releases: Releases,
    clock: TestClock,
    bucket: Bucket,
}

fn setup() -> Setup {
    let home = tempfile::tempdir().unwrap();
    let (releases, clock) = (Releases::default(), TestClock::default());
    releases.publish("v1");
    let bucket = BucketBuilder::new(home.path())
        .provider(releases.clone())
        .clock(clock.clone())
        .build()
        .unwrap();
    Setup {
        home,
        releases,
        clock,
        bucket,
    }
}

fn me() -> Principal {
    "user:test".into()
}

fn default() -> Scope {
    Scope::new("default").unwrap()
}

/// Run a shim with `hello` and return its output line and exit code.
fn run(shim: &Path) -> (String, i32) {
    let output = if cfg!(windows) {
        Command::new("cmd")
            .arg("/C")
            .arg(shim)
            .arg("hello")
            .output()
            .unwrap()
    } else {
        Command::new(shim).arg("hello").output().unwrap()
    };
    (
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        output.status.code().unwrap(),
    )
}

#[test]
fn links_follow_reacquisition_and_shims_launch_through_them() {
    let s = setup();
    let lease = s.bucket.acquire("rel:tool", &default(), &me()).unwrap().lease;
    let linked = s.bucket.link(lease.id, "tool", Some(TOOL), &me()).unwrap();
    assert_eq!(linked.path, s.home.path().join("links").join("tool"));
    let shim: PathBuf = linked.shim.unwrap();
    assert_eq!(
        run(&shim),
        ("v1 hello".into(), 3),
        "arguments, output and exit code pass through"
    );

    s.releases.publish("v2");
    s.bucket.reacquire(lease.id, &me()).unwrap();
    assert_eq!(run(&shim).0, "v2 hello", "the next start runs the new version");
    assert_eq!(s.bucket.links().unwrap().len(), 1);
}

#[test]
fn single_file_content_is_linked_by_name() {
    let s = setup();
    let lease = s.bucket.acquire("raw:tool", &default(), &me()).unwrap().lease;
    let linked = s.bucket.link(lease.id, "raw-tool", Some(TOOL), &me()).unwrap();
    assert!(linked.path.join(TOOL).is_file());
    assert_eq!(run(&linked.shim.unwrap()), ("v1 hello".into(), 3));
}

#[test]
fn names_and_files_are_checked_before_anything_is_created() {
    let s = setup();
    let a = s.bucket.acquire("rel:a", &default(), &me()).unwrap().lease;
    let b = s.bucket.acquire("rel:b", &default(), &me()).unwrap().lease;
    s.bucket.link(a.id, "tool", None, &me()).unwrap();
    let taken = s.bucket.link(b.id, "tool", None, &me()).unwrap_err();
    assert_eq!(taken.code, ErrorCode::Conflict);
    let missing = s.bucket.link(b.id, "other", Some("nope.exe"), &me()).unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
    assert!(!s.home.path().join("links").join("other").exists());
    assert_eq!(s.bucket.links().unwrap().len(), 1);
    assert_eq!(
        s.bucket.link(b.id, "../x", None, &me()).unwrap_err().code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn linked_leases_stay_held_until_released() {
    let s = setup();
    let lease = s.bucket.acquire("rel:tool", &default(), &me()).unwrap().lease;
    let shim = s
        .bucket
        .link(lease.id, "tool", Some(TOOL), &me())
        .unwrap()
        .shim
        .unwrap();
    s.clock.0.fetch_add(30 * 86_400, Ordering::SeqCst);
    let reclaimed = s.bucket.reclaim().unwrap();
    assert!(reclaimed.expired.is_empty() && reclaimed.removed.is_empty());
    assert_eq!(run(&shim).0, "v1 hello");
    s.bucket.release(lease.id, &me()).unwrap();
    assert!(!shim.exists() && !s.home.path().join("links").join("tool").exists());
    assert!(s.bucket.links().unwrap().is_empty());
}

#[test]
fn unlinking_restores_normal_expiry() {
    let s = setup();
    let lease = s.bucket.acquire("rel:tool", &default(), &me()).unwrap().lease;
    s.bucket.link(lease.id, "tool", None, &me()).unwrap();
    s.bucket.unlink("tool", &me()).unwrap();
    s.clock.0.fetch_add(30 * 86_400, Ordering::SeqCst);
    assert_eq!(s.bucket.reclaim().unwrap().expired, [lease.id]);
    assert_eq!(
        s.bucket.unlink("tool", &me()).unwrap_err().code,
        ErrorCode::NotFound
    );
}

#[cfg(windows)]
#[test]
fn content_in_use_is_kept_and_retried() {
    let s = setup();
    let lease = s.bucket.acquire("rel:tool", &default(), &me()).unwrap().lease;
    let old_tree = s
        .bucket
        .link(lease.id, "tool", None, &me())
        .unwrap()
        .path
        .join(TOOL);
    let running = std::fs::File::open(&old_tree).unwrap();
    s.releases.publish("v2");
    s.bucket.reacquire(lease.id, &me()).unwrap();
    let reclaimed = s.bucket.reclaim().unwrap();
    assert_eq!(
        reclaimed.in_use,
        std::slice::from_ref(&lease.digest),
        "the running version is kept"
    );
    drop(running);
    assert_eq!(s.bucket.reclaim().unwrap().removed, [lease.digest]);
}
