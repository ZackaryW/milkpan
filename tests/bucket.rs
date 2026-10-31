//! The acquisition loop, lease settings and extension points, driven through
//! the public API with a fake provider registered the way depending crates do.
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use milkpan::{BeforeReclaim, Bucket, BucketBuilder, Clock, ContentKind, Provider, Resolved, Spec, config};
use panned::{Authorizer, Decision, Error, ErrorCode, Obligation, Principal, Request, Scope, StaticRules};

#[derive(Default)]
struct FakeState {
    versions: HashMap<String, String>,
    failing: HashSet<String>,
    failing_fetch: bool,
    resolves: usize,
    fetches: usize,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<FakeState>>);

impl Fake {
    fn set(&self, source: &str, version: &str) {
        self.0
            .lock()
            .unwrap()
            .versions
            .insert(source.into(), version.into());
    }
    fn fail(&self, source: &str, failing: bool) {
        let mut state = self.0.lock().unwrap();
        if failing {
            state.failing.insert(source.into())
        } else {
            state.failing.remove(source)
        };
    }
    fn counts(&self) -> (usize, usize) {
        let state = self.0.lock().unwrap();
        (state.resolves, state.fetches)
    }
}

impl Provider for Fake {
    fn prefixes(&self) -> Vec<String> {
        vec!["fake".into()]
    }
    fn kind(&self, _: &Spec) -> String {
        "file".into()
    }
    fn resolve(&self, spec: &Spec) -> Result<Resolved, Error> {
        let mut state = self.0.lock().unwrap();
        state.resolves += 1;
        if state.failing.contains(&spec.source) {
            return Err(Error::new(ErrorCode::Unavailable, "network down"));
        }
        let version = state
            .versions
            .get(&spec.source)
            .cloned()
            .unwrap_or_else(|| "v1".into());
        Ok(Resolved {
            version,
            kind: ContentKind::File,
        })
    }
    fn fetch(&self, spec: &Spec, resolved: &Resolved, out: &mut dyn Write) -> Result<(), Error> {
        let mut state = self.0.lock().unwrap();
        if state.failing_fetch {
            return Err(Error::new(ErrorCode::Unavailable, "download dropped"));
        }
        state.fetches += 1;
        // Content depends only on the version, so specs can share blobs.
        let _ = spec;
        out.write_all(format!("content-{}", resolved.version).as_bytes())
            .map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))
    }
}

#[derive(Clone, Default)]
struct TestClock(Arc<AtomicU64>);

impl TestClock {
    fn advance(&self, seconds: u64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now(&self) -> u64 {
        1_000_000 + self.0.load(Ordering::SeqCst)
    }
}

struct Setup {
    home: tempfile::TempDir,
    fake: Fake,
    clock: TestClock,
}

impl Setup {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            fake: Fake::default(),
            clock: TestClock::default(),
        }
    }

    fn builder(&self) -> BucketBuilder {
        BucketBuilder::new(self.home.path())
            .provider(self.fake.clone())
            .clock(self.clock.clone())
    }

    fn bucket(&self) -> Bucket {
        self.builder().build().unwrap()
    }

    fn with_rules(&self, rules: StaticRules) -> Bucket {
        let rules = Arc::new(rules);
        self.builder().authorizer(rules.clone(), rules).build().unwrap()
    }

    fn blobs(&self) -> usize {
        milkpan::Store::new(self.home.path()).digests().unwrap().len()
    }
}

fn scope(name: &str) -> Scope {
    Scope::new(name).unwrap()
}

fn me() -> Principal {
    "user:test".into()
}

fn all(scope: &str) -> StaticRules {
    StaticRules::default()
        .allow(scope, &config::ACTIONS, &["*"])
        .unwrap()
}

#[test]
fn acquisition_fetches_new_versions_once_and_shares_stored_ones() {
    let setup = Setup::new();
    let bucket = setup.with_rules(all("a").allow("b", &config::ACTIONS, &["*"]).unwrap());
    let first = bucket.acquire("fake:tool", &scope("a"), &me()).unwrap();
    assert!(first.fetched);
    assert_eq!(std::fs::read(&first.path).unwrap(), b"content-v1");
    let other = bucket.acquire("fake:tool", &scope("b"), &me()).unwrap();
    assert!(!other.fetched, "the stored version is reused");
    assert_eq!(other.lease.digest, first.lease.digest);
    assert_ne!(other.lease.id, first.lease.id);
    let again = bucket.acquire("fake:tool", &scope("a"), &me()).unwrap();
    assert_eq!(again.lease.id, first.lease.id, "one lease per spec and scope");
    assert_eq!(setup.fake.counts(), (2, 1));
    assert_eq!(bucket.leases(None).unwrap().len(), 2);
}

#[test]
fn renewal_within_the_cadence_needs_no_network() {
    let setup = Setup::new();
    let bucket = setup.bucket();
    let first = bucket.acquire("fake:tool", &scope("default"), &me()).unwrap();
    setup.fake.fail("tool", true);
    setup.clock.advance(600);
    let renewed = bucket.acquire("fake:tool", &scope("default"), &me()).unwrap();
    assert_eq!(renewed.lease.expires_at, first.lease.expires_at + 600);
    assert_eq!(setup.fake.counts().0, 1);
    setup.clock.advance(3_600);
    let stale = bucket.acquire("fake:tool", &scope("default"), &me()).unwrap_err();
    assert_eq!(
        stale.code,
        ErrorCode::Unavailable,
        "past the cadence, acquiring resolves again"
    );
}

#[test]
fn reacquisition_swaps_only_after_the_new_version_is_stored() {
    let setup = Setup::new();
    let bucket = setup.bucket();
    let first = bucket.acquire("fake:tool", &scope("default"), &me()).unwrap();
    setup.fake.set("tool", "v2");
    setup.fake.0.lock().unwrap().failing_fetch = true;
    assert!(bucket.reacquire(first.lease.id, &me()).is_err());
    assert_eq!(
        bucket.lease(first.lease.id).unwrap().version,
        "v1",
        "a failed fetch keeps the old version"
    );
    setup.fake.0.lock().unwrap().failing_fetch = false;
    let swapped = bucket.reacquire(first.lease.id, &me()).unwrap();
    assert_eq!(
        (swapped.lease.id, swapped.lease.version.as_str()),
        (first.lease.id, "v2")
    );
    assert_eq!(setup.blobs(), 1, "the unheld old version is reclaimed");
}

#[test]
fn due_leases_are_reacquired_together_with_each_outcome() {
    let setup = Setup::new();
    let bucket = setup.bucket();
    for name in ["a", "b", "c"] {
        bucket
            .acquire(&format!("fake:{name}"), &scope("default"), &me())
            .unwrap();
    }
    setup.clock.advance(3_601);
    setup.fake.fail("b", true);
    let outcomes = bucket.reacquire_due(&me()).unwrap();
    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes.iter().filter(|(_, r)| r.is_err()).count(), 1);
}

#[test]
fn releasing_reclaims_only_unshared_content() {
    let setup = Setup::new();
    let bucket = setup.with_rules(all("a").allow("b", &config::ACTIONS, &["*"]).unwrap());
    let a = bucket.acquire("fake:tool", &scope("a"), &me()).unwrap();
    let b = bucket.acquire("fake:tool", &scope("b"), &me()).unwrap();
    bucket.release(a.lease.id, &me()).unwrap();
    assert_eq!(setup.blobs(), 1, "still held by scope b");
    bucket.release(b.lease.id, &me()).unwrap();
    assert_eq!(setup.blobs(), 0);
}

#[test]
fn expired_leases_are_dropped_and_their_content_reclaimed() {
    let setup = Setup::new();
    let bucket = setup.bucket();
    let lease = bucket
        .acquire("fake:tool", &scope("default"), &me())
        .unwrap()
        .lease;
    setup.clock.advance(7 * 86_400);
    assert_eq!(
        bucket.path(lease.id, None, &me()).unwrap_err().code,
        ErrorCode::NotFound
    );
    let reclaimed = bucket.reclaim().unwrap();
    assert_eq!((reclaimed.expired, reclaimed.removed.len()), (vec![lease.id], 1));
}

#[test]
fn denied_operations_reach_no_provider_and_change_nothing() {
    let setup = Setup::new();
    let bucket = setup.bucket();
    let denied = bucket.acquire("fake:tool", &scope("ci"), &me()).unwrap_err();
    assert_eq!(denied.code, ErrorCode::Denied);
    assert!(denied.decision.is_some(), "the denial carries evidence");
    assert_eq!(setup.fake.counts(), (0, 0));
    assert!(bucket.leases(None).unwrap().is_empty());
}

#[test]
fn unknown_prefixes_list_the_known_ones() {
    let setup = Setup::new();
    let error = setup
        .bucket()
        .acquire("gitea:o/r", &scope("default"), &me())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("fake"));
}

#[test]
fn obligations_need_a_hook() {
    let setup = Setup::new();
    let scan = StaticRules::default()
        .allow_with("default", &config::ACTIONS, &["*"], vec![Obligation::new("scan")])
        .unwrap();
    let error = setup
        .with_rules(scan)
        .acquire("fake:tool", &scope("default"), &me())
        .unwrap_err();
    assert_eq!(error.code.as_str(), "milkpan.unmet_obligation");
    assert!(setup.with_rules(all("default")).leases(None).unwrap().is_empty());
    let verify = StaticRules::default()
        .allow_with(
            "default",
            &config::ACTIONS,
            &["*"],
            vec![Obligation::new("verify")],
        )
        .unwrap();
    assert!(
        setup
            .with_rules(verify)
            .acquire("fake:tool", &scope("default"), &me())
            .is_ok()
    );
}

#[test]
fn lease_settings_come_from_obligations_per_resource() {
    let setup = Setup::new();
    let ttl = |value: &str| vec![Obligation::new("lease").with("ttl", value)];
    let rules = all("default")
        .allow_with("default", &["acquire"], &["fake:fast-*"], ttl("1d"))
        .unwrap();
    let bucket = setup.with_rules(rules.clone());
    let slow = bucket
        .acquire("fake:slow", &scope("default"), &me())
        .unwrap()
        .lease;
    let fast = bucket
        .acquire("fake:fast-cli", &scope("default"), &me())
        .unwrap()
        .lease;
    assert_eq!(slow.expires_at - slow.resolved_at, 7 * 86_400);
    assert_eq!(fast.expires_at - fast.resolved_at, 86_400);
    let conflict = rules
        .allow_with("default", &["acquire"], &["fake:fast-*"], ttl("7d"))
        .unwrap();
    let error = setup
        .with_rules(conflict)
        .acquire("fake:fast-cli", &scope("default"), &me())
        .unwrap_err();
    assert_eq!(error.code.as_str(), "milkpan.conflicting_obligation");
}

#[test]
fn retained_versions_keep_rollback_copies() {
    let setup = Setup::new();
    let retain = vec![Obligation::new("lease").with("retain", "1")];
    let bucket = setup.with_rules(
        StaticRules::default()
            .allow_with("default", &config::ACTIONS, &["*"], retain)
            .unwrap(),
    );
    let id = bucket
        .acquire("fake:tool", &scope("default"), &me())
        .unwrap()
        .lease
        .id;
    for version in ["v2", "v3"] {
        setup.fake.set("tool", version);
        bucket.reacquire(id, &me()).unwrap();
    }
    assert_eq!(setup.blobs(), 2, "v3 is held, v2 retained, v1 reclaimed");
}

struct Keep;

impl BeforeReclaim for Keep {
    fn allow(&self, _: &str) -> Result<bool, Error> {
        Ok(false)
    }
}

#[test]
fn reclaim_hooks_can_keep_blobs() {
    let setup = Setup::new();
    let bucket = setup.builder().before_reclaim(Keep).build().unwrap();
    let id = bucket
        .acquire("fake:tool", &scope("default"), &me())
        .unwrap()
        .lease
        .id;
    bucket.release(id, &me()).unwrap();
    assert_eq!(setup.blobs(), 1);
    assert_eq!(bucket.reclaim().unwrap().retained_by_hook.len(), 1);
}

struct Counting(AtomicUsize);

impl Authorizer for Counting {
    fn authorize(&self, request: &Request) -> Result<Decision, Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        StaticRules::default()
            .allow("anywhere", &config::ACTIONS, &["*"])?
            .authorize(&Request {
                scope: scope("anywhere"),
                ..request.clone()
            })
    }
}

impl panned::ScopeList for Counting {
    fn scopes(&self) -> Result<Vec<Scope>, Error> {
        Ok(vec![scope("anywhere")])
    }
}

#[test]
fn a_replacement_authorizer_gates_every_operation() {
    let setup = Setup::new();
    let counting = Arc::new(Counting(AtomicUsize::new(0)));
    let bucket = setup
        .builder()
        .authorizer(counting.clone(), counting.clone())
        .build()
        .unwrap();
    let id = bucket.acquire("fake:tool", &scope("ci"), &me()).unwrap().lease.id;
    bucket.path(id, None, &me()).unwrap();
    bucket.reacquire(id, &me()).unwrap();
    bucket.release(id, &me()).unwrap();
    assert_eq!(counting.0.load(Ordering::SeqCst), 4);
}

#[test]
fn configuration_is_optional_but_must_be_valid() {
    let setup = Setup::new();
    let bucket = setup.bucket();
    assert!(bucket.acquire("fake:tool", &scope("default"), &me()).is_ok());
    assert_eq!(
        bucket.acquire("fake:tool", &scope("ci"), &me()).unwrap_err().code,
        ErrorCode::Denied
    );
    std::fs::write(setup.home.path().join(config::FILE), "[[rule]\n").unwrap();
    let error = setup.builder().build().err().unwrap();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(Path::new(&error.details["path"].to_string().trim_matches('"')).ends_with(config::FILE));
}
