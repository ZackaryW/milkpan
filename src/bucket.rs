//! The acquisition loop: authorize, resolve, fetch only what is new, store,
//! fulfil obligations, lease; reacquire with an atomic swap; reclaim what no
//! lease holds.
//!
//! Network work happens outside the state lock. Blobs are immutable, so the
//! lock only covers the state transition and reclamation.
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use panned::{
    Authorizer, Context, Decision, Error, ErrorCode, Principal, Request, Scope, ScopeList, SlotState, Switch,
    core::slot::{Effective, check_switch, unknown_slot},
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    config,
    hooks::{AfterFetch, BeforeReclaim, LeaseHook, Stored, VerifyHook, fulfil},
    links,
    provider::{Provider, Registry},
    spec::Spec,
    state::{
        Clock, Lease, LeaseSettings, LinkRecord, State, StateFile, SystemClock, history_key, version_key,
    },
    store::{ContentKind, Removal, Store},
};

/// The result of acquiring or reacquiring.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Acquired {
    /// The lease as recorded.
    pub lease: Lease,
    /// The local path of the content: a directory for snapshots, a file otherwise.
    pub path: PathBuf,
    /// Whether content was downloaded by this operation.
    pub fetched: bool,
}

/// A link as recorded and where it lives.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Linked {
    /// The link name.
    pub name: String,
    /// The lease it follows.
    pub lease: Lease,
    /// `<home>/links/<name>`.
    pub path: PathBuf,
    /// The launcher in `<home>/bin`, when linked with a file.
    pub shim: Option<PathBuf>,
}

/// Each due lease and the outcome of reacquiring it.
pub type DueOutcomes = Vec<(Uuid, Result<Acquired, Error>)>;

/// What reclamation did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Reclaimed {
    /// Leases dropped because they expired.
    pub expired: Vec<Uuid>,
    /// Blobs deleted.
    pub removed: Vec<String>,
    /// Blobs a before-reclaim hook kept.
    pub retained_by_hook: Vec<String>,
    /// Blobs kept because the platform reported them in use; retried next time.
    pub in_use: Vec<String>,
    /// Staging entries older than a day removed from `tmp/`.
    pub stale_staging: usize,
}

/// Assembles a [`Bucket`].
pub struct BucketBuilder {
    home: PathBuf,
    providers: Registry,
    policy: Option<(Arc<dyn Authorizer>, Arc<dyn ScopeList>)>,
    after: Vec<Arc<dyn AfterFetch>>,
    before: Vec<Arc<dyn BeforeReclaim>>,
    clock: Arc<dyn Clock>,
    lock_wait: Option<Duration>,
}

impl BucketBuilder {
    /// A builder for the bucket at `home`, with no providers and the policy
    /// from `<home>/milkpan.toml`.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            providers: Registry::default(),
            policy: None,
            after: Vec::new(),
            before: Vec::new(),
            clock: Arc::new(SystemClock),
            lock_wait: None,
        }
    }

    /// Add a provider; it wins over earlier providers for the same prefix.
    pub fn provider(mut self, provider: impl Provider + 'static) -> Self {
        self.providers.register(Arc::new(provider));
        self
    }

    /// Replace the configured policy with another authorizer and scope list.
    pub fn authorizer(mut self, authorizer: Arc<dyn Authorizer>, scopes: Arc<dyn ScopeList>) -> Self {
        self.policy = Some((authorizer, scopes));
        self
    }

    /// Add an after-fetch hook, consulted before the built-in ones.
    pub fn after_fetch(mut self, hook: impl AfterFetch + 'static) -> Self {
        self.after.push(Arc::new(hook));
        self
    }

    /// Add a before-reclaim hook.
    pub fn before_reclaim(mut self, hook: impl BeforeReclaim + 'static) -> Self {
        self.before.push(Arc::new(hook));
        self
    }

    /// Use another clock (for tests).
    pub fn clock(mut self, clock: impl Clock + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// How long writers wait for the bucket lock.
    pub fn lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = Some(wait);
        self
    }

    /// Build the bucket; fails when the configuration file is invalid.
    pub fn build(self) -> Result<Bucket, Error> {
        let (authorizer, scopes) = match self.policy {
            Some(policy) => policy,
            None => {
                let rules = Arc::new(config::load(&self.home)?);
                (rules.clone() as Arc<dyn Authorizer>, rules as Arc<dyn ScopeList>)
            }
        };
        let mut after = self.after;
        after.push(Arc::new(VerifyHook));
        after.push(Arc::new(LeaseHook));
        let mut state = StateFile::new(&self.home);
        if let Some(wait) = self.lock_wait {
            state = state.with_lock_wait(wait);
        }
        Ok(Bucket {
            store: Store::new(&self.home),
            state,
            home: self.home,
            providers: self.providers,
            authorizer,
            scopes,
            after,
            before: self.before,
            clock: self.clock,
        })
    }
}

/// A resource bucket.
pub struct Bucket {
    home: PathBuf,
    store: Store,
    state: StateFile,
    providers: Registry,
    authorizer: Arc<dyn Authorizer>,
    scopes: Arc<dyn ScopeList>,
    after: Vec<Arc<dyn AfterFetch>>,
    before: Vec<Arc<dyn BeforeReclaim>>,
    clock: Arc<dyn Clock>,
}

impl Bucket {
    /// The bucket's home directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The authorizer every lease operation goes through.
    pub fn authorizer(&self) -> Arc<dyn Authorizer> {
        self.authorizer.clone()
    }

    /// The scopes the policy knows.
    pub fn scopes(&self) -> Arc<dyn ScopeList> {
        self.scopes.clone()
    }

    fn authorize(
        &self,
        principal: &Principal,
        scope: &Scope,
        action: &str,
        spec: &Spec,
        provider: &dyn Provider,
    ) -> Result<Decision, Error> {
        let request = Request {
            principal: principal.clone(),
            scope: scope.clone(),
            action: action.into(),
            resource: spec.canonical().as_str().into(),
            context: Context::new()
                .with("source.provider", spec.prefix.as_str())
                .with("source.kind", provider.kind(spec)),
        };
        let decision = self.authorizer.authorize(&request)?;
        if decision.allowed() {
            Ok(decision)
        } else {
            Err(Error::denied(decision))
        }
    }

    fn settings(&self, spec: &Spec, digest: &str, decision: &Decision) -> Result<LeaseSettings, Error> {
        let mut settings = LeaseSettings::default();
        let stored = Stored {
            store: &self.store,
            spec,
            digest,
        };
        fulfil(&self.after, &stored, &decision.obligations, Some(&mut settings))?;
        Ok(settings)
    }

    /// The directory a link to `lease` points at: the tree of an archive, or a
    /// named view of a single file (named after `file`, the spec path or the asset).
    fn content_dir(&self, lease: &Lease, file: Option<&str>) -> Result<PathBuf, Error> {
        match lease.kind {
            ContentKind::Archive => self.store.tree(&lease.digest),
            ContentKind::File => {
                let spec_name = lease
                    .spec
                    .path
                    .as_deref()
                    .or(lease.spec.asset.as_deref())
                    .and_then(|p| p.rsplit('/').next())
                    .filter(|n| !n.contains('*'));
                let name = file.or(spec_name).unwrap_or("content");
                self.store.file_view(&lease.digest, name, file.is_some())
            }
        }
    }

    /// Point every link following `lease` at its current content.
    fn repoint(&self, state: &State, lease: &Lease) -> Result<(), Error> {
        for (name, record) in state.links_of(lease.id) {
            let dir = self.content_dir(lease, record.file.as_deref())?;
            links::point(&self.home, &name, &dir)?;
        }
        Ok(())
    }

    fn content_path(&self, kind: ContentKind, digest: &str) -> Result<PathBuf, Error> {
        match kind {
            ContentKind::File => Ok(self.store.blob_path(digest)),
            ContentKind::Archive => self.store.tree(digest),
        }
    }

    /// Acquire `spec` for `scope`: renew without network access when the last
    /// resolution is within the lease's refresh cadence, otherwise resolve and
    /// fetch if new.
    pub fn acquire(&self, spec: &str, scope: &Scope, principal: &Principal) -> Result<Acquired, Error> {
        let (spec, provider) = self.providers.spec(spec)?;
        let decision = self.authorize(principal, scope, "acquire", &spec, provider.as_ref())?;
        let now = self.clock.now();
        let existing = self.state.read()?.lease_for(scope, &spec).cloned();
        if let Some(lease) = existing
            && now < lease.resolved_at.saturating_add(lease.settings.refresh)
            && lease.expires_at > now
            && self.store.has(&lease.digest)
        {
            let settings = self.settings(&spec, &lease.digest, &decision)?;
            let lease = self.state.transact(|state| {
                let lease = state
                    .leases
                    .iter_mut()
                    .find(|l| l.id == lease.id)
                    .ok_or_else(|| lease_gone(lease.id))?;
                lease.settings = settings;
                lease.expires_at = now + settings.ttl;
                lease.principal = principal.clone();
                Ok(lease.clone())
            })?;
            let path = self.content_path(lease.kind, &lease.digest)?;
            return Ok(Acquired {
                lease,
                path,
                fetched: false,
            });
        }
        self.refresh(spec, provider.as_ref(), scope, principal, &decision)
    }

    fn refresh(
        &self,
        spec: Spec,
        provider: &dyn Provider,
        scope: &Scope,
        principal: &Principal,
        decision: &Decision,
    ) -> Result<Acquired, Error> {
        let resolved = provider.resolve(&spec)?;
        let known = self
            .state
            .read()?
            .digest_for(&spec, &resolved.version)
            .map(str::to_string);
        let (digest, fetched) = match known.filter(|d| self.store.has(d)) {
            Some(digest) => (digest, false),
            None => (
                self.store.ingest(|out| provider.fetch(&spec, &resolved, out))?,
                true,
            ),
        };
        let path = self.content_path(resolved.kind, &digest)?;
        let settings = self.settings(&spec, &digest, decision)?;
        let now = self.clock.now();
        let (lease, swapped) = self.state.transact(|state| {
            let previous = state.lease_for(scope, &spec).cloned();
            let lease = Lease {
                id: previous.as_ref().map_or_else(Uuid::new_v4, |l| l.id),
                spec: spec.clone(),
                scope: scope.clone(),
                principal: principal.clone(),
                version: resolved.version.clone(),
                digest: digest.clone(),
                kind: resolved.kind,
                resolved_at: now,
                expires_at: now + settings.ttl,
                settings,
            };
            state
                .versions
                .insert(version_key(&spec, &resolved.version), digest.clone());
            state.upsert(lease.clone());
            let swapped = previous.is_some_and(|p| p.version != resolved.version);
            if swapped {
                // Links follow the swap; the content they pointed at before stays untouched.
                self.repoint(state, &lease)?;
            }
            Ok((lease, swapped))
        })?;
        if swapped {
            // The swap is committed; reclamation failing must not fail the acquisition.
            let _ = self.reclaim();
        }
        Ok(Acquired { lease, path, fetched })
    }

    fn live(&self, id: Uuid) -> Result<(Lease, Arc<dyn Provider>), Error> {
        let state = self.state.read()?;
        let lease = state.lease(id).cloned().ok_or_else(|| lease_gone(id))?;
        // Linked leases do not expire.
        if lease.expires_at <= self.clock.now() && state.links_of(id).is_empty() {
            return Err(lease_gone(id).with_detail("expired", true));
        }
        let provider = self.providers.get(&lease.spec.prefix)?;
        Ok((lease, provider))
    }

    /// Resolve a lease's selector again and swap it to the new version, if any.
    pub fn reacquire(&self, id: Uuid, principal: &Principal) -> Result<Acquired, Error> {
        let (lease, provider) = self.live(id)?;
        let decision = self.authorize(
            principal,
            &lease.scope,
            "reacquire",
            &lease.spec,
            provider.as_ref(),
        )?;
        self.refresh(lease.spec, provider.as_ref(), &lease.scope, principal, &decision)
    }

    /// Reacquire every live lease whose refresh cadence has elapsed.
    pub fn reacquire_due(&self, principal: &Principal) -> Result<DueOutcomes, Error> {
        let now = self.clock.now();
        let state = self.state.read()?;
        let due: Vec<Uuid> = state
            .leases
            .iter()
            .filter(|l| l.expires_at > now || !state.links_of(l.id).is_empty())
            .filter(|l| now >= l.resolved_at.saturating_add(l.settings.refresh))
            .map(|l| l.id)
            .collect();
        Ok(due
            .into_iter()
            .map(|id| (id, self.reacquire(id, principal)))
            .collect())
    }

    /// Release a lease and reclaim what it held.
    pub fn release(&self, id: Uuid, principal: &Principal) -> Result<Lease, Error> {
        let (lease, provider) = self.live(id)?;
        self.authorize(principal, &lease.scope, "release", &lease.spec, provider.as_ref())?;
        let released = self.state.transact(|state| {
            for (name, _) in state.links_of(id) {
                links::remove(&self.home, &name)?;
                state.links.remove(&name);
            }
            state.remove(id).ok_or_else(|| lease_gone(id))
        })?;
        let _ = self.reclaim();
        Ok(released)
    }

    /// The local path of a lease's content, or of `file` inside a snapshot.
    pub fn path(&self, id: Uuid, file: Option<&str>, principal: &Principal) -> Result<PathBuf, Error> {
        let (lease, provider) = self.live(id)?;
        let decision = self.authorize(principal, &lease.scope, "read", &lease.spec, provider.as_ref())?;
        let stored = Stored {
            store: &self.store,
            spec: &lease.spec,
            digest: &lease.digest,
        };
        fulfil(&self.after, &stored, &decision.obligations, None)?;
        let root = self.content_path(lease.kind, &lease.digest)?;
        let Some(file) = file else { return Ok(root) };
        if lease.kind != ContentKind::Archive {
            return Err(
                Error::new(ErrorCode::InvalidRequest, "only snapshot leases contain files")
                    .with_detail("lease", lease.id.to_string()),
            );
        }
        let relative = Path::new(file);
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(
                Error::new(ErrorCode::InvalidRequest, format!("unsafe path {file:?}"))
                    .with_detail("path", file),
            );
        }
        let path = root.join(relative);
        if !path.exists() {
            return Err(
                Error::new(ErrorCode::NotFound, format!("{file} is not in the snapshot"))
                    .with_detail("path", file)
                    .with_detail("lease", lease.id.to_string()),
            );
        }
        Ok(path)
    }

    /// Link a lease under `name`, and with `file` also write the launcher
    /// `<home>/bin/<name>`. The link follows every later version of the lease;
    /// linking renews the lease, and linked leases do not expire.
    pub fn link(
        &self,
        id: Uuid,
        name: &str,
        file: Option<&str>,
        principal: &Principal,
    ) -> Result<Linked, Error> {
        links::validate_name(name)?;
        let file = file.map(links::validate_file).transpose()?;
        let (lease, provider) = self.live(id)?;
        self.authorize(principal, &lease.scope, "link", &lease.spec, provider.as_ref())?;
        let file_text = file.as_ref().map(|f| f.to_string_lossy().replace('\\', "/"));
        let dir = self.content_dir(&lease, file_text.as_deref())?;
        if let Some(file) = &file
            && !dir.join(file).is_file()
        {
            return Err(Error::new(
                ErrorCode::NotFound,
                format!("{} is not in the lease's content", file.display()),
            )
            .with_detail("file", file.to_string_lossy().into_owned())
            .with_detail("lease", id.to_string()));
        }
        let now = self.clock.now();
        self.state.transact(|state| {
            if let Some(existing) = state.links.get(name)
                && existing.lease != id
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!("link {name} belongs to lease {}", existing.lease),
                )
                .with_detail("name", name)
                .with_detail("lease", existing.lease.to_string()));
            }
            let lease = state
                .leases
                .iter_mut()
                .find(|l| l.id == id)
                .ok_or_else(|| lease_gone(id))?;
            lease.expires_at = now + lease.settings.ttl;
            let lease = lease.clone();
            let path = links::point(&self.home, name, &dir)?;
            links::remove_shims(&self.home, name)?;
            let shim = file
                .as_ref()
                .map(|f| links::write_shim(&self.home, name, f))
                .transpose()?;
            state.links.insert(
                name.into(),
                LinkRecord {
                    lease: id,
                    file: file_text.clone(),
                },
            );
            Ok(Linked {
                name: name.into(),
                lease,
                path,
                shim,
            })
        })
    }

    /// Remove a link and its launcher. The lease stays, with normal expiry once unlinked.
    pub fn unlink(&self, name: &str, principal: &Principal) -> Result<LinkRecord, Error> {
        let state = self.state.read()?;
        let record = state.links.get(name).cloned().ok_or_else(|| unknown_link(name))?;
        if let Some(lease) = state.lease(record.lease) {
            let provider = self.providers.get(&lease.spec.prefix)?;
            self.authorize(principal, &lease.scope, "link", &lease.spec, provider.as_ref())?;
        }
        self.state.transact(|state| {
            let record = state.links.remove(name).ok_or_else(|| unknown_link(name))?;
            links::remove(&self.home, name)?;
            Ok(record)
        })
    }

    /// Every link with the lease it follows.
    pub fn links(&self) -> Result<Vec<Linked>, Error> {
        let state = self.state.read()?;
        let bin = self.home.join("bin");
        Ok(state
            .links
            .iter()
            .filter_map(|(name, record)| {
                let lease = state.lease(record.lease)?.clone();
                let shim = record.file.as_ref().map(|_| {
                    if cfg!(windows) {
                        bin.join(format!("{name}.cmd"))
                    } else {
                        bin.join(name)
                    }
                });
                Some(Linked {
                    name: name.clone(),
                    lease,
                    path: links::link_path(&self.home, name),
                    shim,
                })
            })
            .collect())
    }

    fn slot_of(&self, state: &State, name: &str) -> Result<(SlotState, Lease), Error> {
        let record = state.links.get(name).ok_or_else(|| unknown_slot(name))?;
        let lease = state
            .lease(record.lease)
            .cloned()
            .ok_or_else(|| unknown_slot(name))?;
        let mut available = vec![lease.version.clone()];
        if let Some(history) = state.history.get(&history_key(&lease.scope, &lease.spec)) {
            for version in history.iter().rev() {
                if state
                    .digest_for(&lease.spec, version)
                    .is_some_and(|d| self.store.has(d))
                {
                    available.push(version.clone());
                }
            }
        }
        let slot = SlotState {
            name: name.into(),
            current: Some(lease.version.clone()),
            available,
        };
        Ok((slot, lease))
    }

    /// Every link as a panned slot: the lease's version plus its retained versions.
    pub fn slots(&self) -> Result<Vec<SlotState>, Error> {
        let state = self.state.read()?;
        state
            .links
            .keys()
            .map(|name| self.slot_of(&state, name).map(|(slot, _)| slot))
            .collect()
    }

    /// One link as a panned slot.
    pub fn slot(&self, name: &str) -> Result<SlotState, Error> {
        self.slot_of(&self.state.read()?, name).map(|(slot, _)| slot)
    }

    /// Point a link and its lease at one of the lease's retained versions. The
    /// spec's selector is unchanged, so a later reacquisition may move it forward.
    pub fn switch(&self, name: &str, version: &str, principal: &Principal) -> Result<Switch, Error> {
        let (slot, lease) = self.slot_of(&self.state.read()?, name)?;
        let switch = check_switch(&slot, version, Effective::NextRun)?;
        if !switch.changed {
            return Ok(switch);
        }
        let provider = self.providers.get(&lease.spec.prefix)?;
        self.authorize(principal, &lease.scope, "link", &lease.spec, provider.as_ref())?;
        self.state.transact(|state| {
            let current = state
                .lease(lease.id)
                .cloned()
                .ok_or_else(|| lease_gone(lease.id))?;
            let digest = state
                .digest_for(&current.spec, version)
                .map(str::to_string)
                .filter(|d| self.store.has(d))
                .ok_or_else(|| {
                    check_switch(
                        &SlotState {
                            available: vec![],
                            ..slot.clone()
                        },
                        version,
                        Effective::NextRun,
                    )
                    .unwrap_err()
                })?;
            let switched = Lease {
                version: version.into(),
                digest,
                ..current
            };
            state.upsert(switched.clone());
            self.repoint(state, &switched)
        })?;
        Ok(switch)
    }

    /// Leases, optionally only one scope's.
    pub fn leases(&self, scope: Option<&Scope>) -> Result<Vec<Lease>, Error> {
        let state = self.state.read()?;
        Ok(state
            .leases
            .into_iter()
            .filter(|l| scope.is_none_or(|s| &l.scope == s))
            .collect())
    }

    /// One lease by id.
    pub fn lease(&self, id: Uuid) -> Result<Lease, Error> {
        self.state
            .read()?
            .lease(id)
            .cloned()
            .ok_or_else(|| lease_gone(id))
    }

    /// The lease a scope holds on a spec.
    pub fn find(&self, spec: &str, scope: &Scope) -> Result<Lease, Error> {
        let (spec, _) = self.providers.spec(spec)?;
        self.state
            .read()?
            .lease_for(scope, &spec)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    format!("scope {scope} holds no lease on {spec}"),
                )
                .with_detail("spec", spec.canonical())
                .with_detail("scope", scope.as_str())
            })
    }

    /// Drop expired leases and delete every blob no live lease or retained
    /// version protects, unless a before-reclaim hook keeps it.
    pub fn reclaim(&self) -> Result<Reclaimed, Error> {
        let now = self.clock.now();
        self.state.transact(|state| {
            let mut reclaimed = Reclaimed {
                stale_staging: self.store.clear_stale_tmp(Duration::from_secs(86_400))?,
                ..Reclaimed::default()
            };
            let expired: Vec<Uuid> = state
                .leases
                .iter()
                .filter(|l| l.expires_at <= now && !state.links.values().any(|r| r.lease == l.id))
                .map(|l| l.id)
                .collect();
            for id in &expired {
                state.remove(*id);
            }
            reclaimed.expired = expired;
            let protected = protected(state);
            for digest in self.store.digests()? {
                if protected.contains(&digest) {
                    continue;
                }
                let mut keep = false;
                for hook in &self.before {
                    if !hook.allow(&digest)? {
                        keep = true;
                    }
                }
                if keep {
                    reclaimed.retained_by_hook.push(digest);
                    continue;
                }
                match self.store.remove(&digest)? {
                    Removal::Removed => reclaimed.removed.push(digest),
                    Removal::InUse => reclaimed.in_use.push(digest),
                }
            }
            let stored: BTreeSet<String> = self.store.digests()?.into_iter().collect();
            state.versions.retain(|_, digest| stored.contains(digest));
            Ok(reclaimed)
        })
    }
}

/// Digests held by live leases or by their retained previous versions.
fn protected(state: &mut State) -> BTreeSet<String> {
    let mut protected = BTreeSet::new();
    for lease in &state.leases {
        protected.insert(lease.digest.clone());
        let key = history_key(&lease.scope, &lease.spec);
        if let Some(history) = state.history.get_mut(&key) {
            let keep = history.len().saturating_sub(lease.settings.retain as usize);
            history.drain(..keep);
            for version in history.iter() {
                if let Some(digest) = state.versions.get(&version_key(&lease.spec, version)) {
                    protected.insert(digest.clone());
                }
            }
        }
    }
    protected
}

fn unknown_link(name: &str) -> Error {
    Error::new(ErrorCode::NotFound, format!("no link named {name:?}")).with_detail("name", name)
}

fn lease_gone(id: Uuid) -> Error {
    Error::new(ErrorCode::NotFound, format!("no live lease {id}")).with_detail("lease", id.to_string())
}
