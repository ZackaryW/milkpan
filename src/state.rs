//! Leases and resolution records in one JSON document, changed only under an
//! exclusive lock and replaced atomically.
use std::{
    collections::BTreeMap,
    fs::{self, File, TryLockError},
    io::Write,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use panned::{Error, ErrorCode, Principal, Scope};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    spec::Spec,
    store::{ContentKind, io_error},
};

/// Seconds since the Unix epoch, injectable for tests.
pub trait Clock: Send + Sync {
    /// The current time in seconds since the Unix epoch.
    fn now(&self) -> u64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Lifetime settings applied to a lease, from its `lease` obligations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseSettings {
    /// Seconds from the last renewal until the lease expires.
    pub ttl: u64,
    /// Seconds a resolution stays fresh before acquiring resolves again.
    pub refresh: u64,
    /// Previous versions kept from reclamation.
    pub retain: u32,
}

impl Default for LeaseSettings {
    fn default() -> Self {
        Self {
            ttl: 7 * 86_400,
            refresh: 3_600,
            retain: 0,
        }
    }
}

/// One scope holding one version of one spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    /// Stable across renewals and reacquisitions.
    pub id: Uuid,
    /// What is held.
    pub spec: Spec,
    /// Who holds it.
    pub scope: Scope,
    /// Who last acquired or renewed it.
    pub principal: Principal,
    /// The exact version held.
    pub version: String,
    /// The blob holding that version.
    pub digest: String,
    /// How the content is presented.
    pub kind: ContentKind,
    /// When the selector was last resolved.
    pub resolved_at: u64,
    /// When the lease expires unless renewed.
    pub expires_at: u64,
    /// The settings applied at the last renewal.
    pub settings: LeaseSettings,
}

/// A stable link to a lease's content, with the file its shim launches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRecord {
    /// The lease the link follows.
    pub lease: Uuid,
    /// The file inside the content that `<home>/bin/<name>` launches, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// The bucket's recorded state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Current leases.
    #[serde(default)]
    pub leases: Vec<Lease>,
    /// Versions previously leased, oldest first, keyed by `"<scope> <spec>"`.
    #[serde(default)]
    pub history: BTreeMap<String, Vec<String>>,
    /// Known digests, keyed by `"<spec> <version>"`.
    #[serde(default)]
    pub versions: BTreeMap<String, String>,
    /// Links by name.
    #[serde(default)]
    pub links: BTreeMap<String, LinkRecord>,
}

pub(crate) fn history_key(scope: &Scope, spec: &Spec) -> String {
    format!("{scope} {spec}")
}

pub(crate) fn version_key(spec: &Spec, version: &str) -> String {
    format!("{spec} {version}")
}

impl State {
    /// The lease a scope holds on a spec.
    pub fn lease_for(&self, scope: &Scope, spec: &Spec) -> Option<&Lease> {
        self.leases.iter().find(|l| &l.scope == scope && &l.spec == spec)
    }

    /// A lease by id.
    pub fn lease(&self, id: Uuid) -> Option<&Lease> {
        self.leases.iter().find(|l| l.id == id)
    }

    /// The links following a lease.
    pub fn links_of(&self, id: Uuid) -> Vec<(String, LinkRecord)> {
        self.links
            .iter()
            .filter(|(_, l)| l.lease == id)
            .map(|(n, l)| (n.clone(), l.clone()))
            .collect()
    }

    /// The stored digest of a spec's version, if fetched before.
    pub fn digest_for(&self, spec: &Spec, version: &str) -> Option<&str> {
        self.versions.get(&version_key(spec, version)).map(String::as_str)
    }

    /// Insert or replace the scope's lease on the spec; a replaced version moves into history.
    pub fn upsert(&mut self, lease: Lease) {
        match self
            .leases
            .iter_mut()
            .find(|l| l.scope == lease.scope && l.spec == lease.spec)
        {
            Some(existing) => {
                if existing.version != lease.version {
                    let key = history_key(&existing.scope, &existing.spec);
                    let history = self.history.entry(key).or_default();
                    history.retain(|v| v != &existing.version && v != &lease.version);
                    history.push(existing.version.clone());
                }
                *existing = lease;
            }
            None => self.leases.push(lease),
        }
        self.versions_for_leases();
    }

    fn versions_for_leases(&mut self) {
        for lease in &self.leases {
            self.versions
                .insert(version_key(&lease.spec, &lease.version), lease.digest.clone());
        }
    }

    /// Remove a lease and its history.
    pub fn remove(&mut self, id: Uuid) -> Option<Lease> {
        let index = self.leases.iter().position(|l| l.id == id)?;
        let lease = self.leases.remove(index);
        self.history.remove(&history_key(&lease.scope, &lease.spec));
        Some(lease)
    }
}

/// The state file and its lock under a bucket home.
#[derive(Debug, Clone)]
pub struct StateFile {
    root: PathBuf,
    lock_wait: Duration,
}

impl StateFile {
    /// The state of the bucket at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock_wait: Duration::from_secs(60),
        }
    }

    /// How long a writer waits for the lock before failing with `conflict`.
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }

    fn path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// The last committed state; empty when none was written yet.
    pub fn read(&self) -> Result<State, Error> {
        let path = self.path();
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                Error::new(
                    ErrorCode::Internal,
                    format!("corrupt state {}: {e}", path.display()),
                )
                .with_detail("path", path.display().to_string())
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(io_error(&path, e)),
        }
    }

    /// Change the state under the exclusive lock. Nothing is written when `change` fails.
    pub fn transact<T>(&self, change: impl FnOnce(&mut State) -> Result<T, Error>) -> Result<T, Error> {
        fs::create_dir_all(&self.root).map_err(|e| io_error(&self.root, e))?;
        let lock_path = self.root.join("lock");
        let lock = File::create(&lock_path).map_err(|e| io_error(&lock_path, e))?;
        let started = Instant::now();
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if started.elapsed() < self.lock_wait => {
                    thread::sleep(Duration::from_millis(20))
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "another milkpan process holds the bucket lock",
                    )
                    .with_detail("path", lock_path.display().to_string()));
                }
                Err(TryLockError::Error(e)) => return Err(io_error(&lock_path, e)),
            }
        }
        let mut state = self.read()?;
        let result = change(&mut state)?;
        self.write(&state)?;
        Ok(result)
    }

    fn write(&self, state: &State) -> Result<(), Error> {
        let path = self.path();
        let tmp = self.root.join(format!("state.json.{}", Uuid::new_v4()));
        let text =
            serde_json::to_vec_pretty(state).map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))?;
        let mut file = File::create(&tmp).map_err(|e| io_error(&tmp, e))?;
        file.write_all(&text)
            .and_then(|()| file.sync_all())
            .map_err(|e| io_error(&tmp, e))?;
        drop(file);
        fs::rename(&tmp, &path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            io_error(&path, e)
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    pub(crate) fn lease(scope: &str, spec: &str, version: &str) -> Lease {
        Lease {
            id: Uuid::new_v4(),
            spec: Spec::parse(spec).unwrap(),
            scope: Scope::new(scope).unwrap(),
            principal: "user:test".into(),
            version: version.into(),
            digest: format!("{:0>64}", version),
            kind: ContentKind::File,
            resolved_at: 0,
            expires_at: 100,
            settings: LeaseSettings::default(),
        }
    }

    #[test]
    fn concurrent_writers_do_not_lose_leases() {
        let home = tempfile::tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let threads: Vec<_> = ["github:o/a", "github:o/b"]
            .into_iter()
            .map(|spec| {
                let (file, barrier) = (StateFile::new(home.path()), barrier.clone());
                thread::spawn(move || {
                    barrier.wait();
                    file.transact(|state| {
                        thread::sleep(Duration::from_millis(50));
                        state.upsert(lease("ci", spec, "1"));
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(StateFile::new(home.path()).read().unwrap().leases.len(), 2);
    }

    #[test]
    fn failed_or_interrupted_writes_keep_the_previous_state() {
        let home = tempfile::tempdir().unwrap();
        let file = StateFile::new(home.path());
        file.transact(|state| {
            state.upsert(lease("ci", "github:o/a", "1"));
            Ok(())
        })
        .unwrap();
        let before = file.read().unwrap();
        let failed: Result<(), _> = file.transact(|state| {
            state.leases.clear();
            Err(Error::new(ErrorCode::Unavailable, "fetch failed"))
        });
        assert!(failed.is_err());
        // A crash mid-write leaves only a stray temporary file.
        fs::write(home.path().join("state.json.partial"), b"{\"leases\": [").unwrap();
        assert_eq!(file.read().unwrap(), before);
    }

    #[test]
    fn a_held_lock_times_out_with_conflict() {
        let home = tempfile::tempdir().unwrap();
        let file = StateFile::new(home.path()).with_lock_wait(Duration::from_millis(50));
        let error = file.transact(|_| file.transact(|_| Ok(()))).unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
    }

    #[test]
    fn swapping_a_version_records_history() {
        let mut state = State::default();
        let first = lease("ci", "github:o/a", "1");
        state.upsert(first.clone());
        state.upsert(Lease {
            version: "2".into(),
            digest: format!("{:0>64}", 2),
            ..first.clone()
        });
        state.upsert(Lease {
            version: "3".into(),
            digest: format!("{:0>64}", 3),
            ..first.clone()
        });
        let key = history_key(&first.scope, &first.spec);
        assert_eq!(state.history[&key], ["1", "2"]);
        assert_eq!(state.leases.len(), 1);
        assert_eq!(
            state.digest_for(&first.spec, "1"),
            Some(format!("{:0>64}", 1).as_str())
        );
        state.remove(first.id);
        assert!(state.leases.is_empty() && !state.history.contains_key(&key));
    }
}
