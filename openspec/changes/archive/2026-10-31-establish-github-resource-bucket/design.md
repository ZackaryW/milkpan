# Design

## Context

See proposal.md (Why). Milkpan is an empty Rust repository. Panned (`F:\(WORK) git\panned`, archived change `establish-core-and-cli-contract`) provides what milkpan builds on:
- the core types `Principal`, `Scope`, `Request`, `Context` and `Decision`, plus the `Authorizer` and `ScopeList` traits and `StaticRules`;
- the CLI contract: `Runner`, `CommandSet`, `GenericCommands`, `Error`/`ErrorCode` and the envelope.

Without a handshake, `GenericCommands` exposes only `scope list` and `explain`, which is what milkpan uses. Core `StaticRules` rules carry no obligations today; panned's `static-rules-obligations` change adds an optional obligation list per rule, which milkpan needs for lease settings. Neither repository has a remote yet.

## Goals / Non-Goals

**Goals:**
- One acquisition loop, readable in one file, that every provider goes through.
- Crash safety from two primitives: immutable content-addressed blobs and atomically replaced state.
- A library surface Saucepan extends (providers, authorizer, hooks, command set) without forking milkpan.

**Non-Goals:**
- Git protocol, partial clones or subdirectory snapshots: a snapshot is the whole tree at one commit.
- GitHub Enterprise authentication flows. The API base URL is overridable, which makes it usable but untested.
- Authorizing `reclaim`: it is local maintenance that deletes only unheld content.

## Decisions

### Crate layout
There is one crate with a library and the `milkpan` binary. It depends on `panned` with default features (core and cli, no engine). Until panned has a remote, it is a path dependency (`../panned`), and CI checks out panned beside milkpan.

The modules are:
- `spec`: parse and canonicalize specs.
- `store`: blobs and trees.
- `state`: leases and the lock.
- `provider`: the trait, a registry and `github`.
- `bucket`: the loop.
- `config`, `hooks` and `cli`.

Alternative: a workspace with separate library and binary crates. Rejected because Saucepan needs both and one crate is simpler to version.

### On-disk layout and atomicity
```
<home>/milkpan.toml            optional configuration
<home>/state.json              leases and resolution records
<home>/lock                    exclusive lock for writers
<home>/blobs/sha256/<2>/<62>   immutable content
<home>/trees/<digest>/         extracted snapshots, derived from blobs
<home>/tmp/                    staging for downloads and extraction
```
- **Downloads** stream into `tmp/` while hashing, then are renamed into `blobs/`. If the blob already exists, the temporary file is dropped.
- **Snapshots** are extracted into `tmp/` and renamed into `trees/`.
- **State changes** take an exclusive file lock (`std::fs::File::try_lock`, Rust 1.89+, waiting up to 60 s, then `conflict`). They read `state.json`, apply the change, write a temporary file and rename it over `state.json`.
- **Network work** happens before the lock is taken: resolve, fetch and store are safe without it because blobs are immutable. The lock covers only the state transition and reclamation, so a slow download never blocks other processes.

An interrupted process leaves at worst orphan files in `tmp/` (cleared by `reclaim`) or an unreferenced blob (reclaimed later).

Alternative: SQLite for state. Rejected because one small JSON document under a lock is enough at this scale and avoids a native dependency.

### Acquisition loop
- **`acquire(spec, scope, principal)`:**
  1. Authorize `acquire`.
  2. If a lease exists and its last resolution is within the cadence, renew it under the lock.
  3. Otherwise resolve, fetch if the digest for that version is unknown, store, run after-fetch hooks with the decision's obligations, then record the lease under the lock.
- **`reacquire`:** the same as `acquire` but always resolves. The lease id survives; only the version, resolution time and expiry change.
- **Reclamation** runs after `release`, after a version swap and on `reclaim`:
  1. Drop expired leases.
  2. Compute the protected set: versions held by current leases, plus each (scope, spec)'s N most recent previous versions.
  3. Ask the before-reclaim hooks, then delete the remaining blobs and their trees.

State keeps a per-(scope, spec) history of versions for retention, and a version-to-digest index so known versions are never re-downloaded.

### GitHub provider
These are REST endpoints over `ureq` with rustls. The base URL comes from `MILKPAN_GITHUB_API` (default `https://api.github.com`), which lets tests use a local mock server.

| Kind | Resolve | Fetch | Version |
| --- | --- | --- | --- |
| snapshot | `GET /repos/{o}/{r}` (default branch), `GET /repos/{o}/{r}/commits/{ref}` | `GET /repos/{o}/{r}/tarball/{sha}` | commit SHA |
| file | same as snapshot | `GET /repos/{o}/{r}/contents/{path}?ref={sha}` with `Accept: application/vnd.github.raw` | commit SHA (the spec carries the path) |
| release asset | `GET /repos/{o}/{r}/releases/tags/{tag}` or `/releases/latest`, then match the asset name | `GET /repos/{o}/{r}/releases/assets/{id}` with `Accept: application/octet-stream` | asset id |

Snapshot extraction:
- strips the tarball's top-level directory;
- rejects absolute paths and `..` components;
- skips links and every other entry that is neither a file nor a directory, so nothing can point outside the tree.

The token is read from `GH_TOKEN`, `GITHUB_TOKEN`, then `gh auth token` (a subprocess, run once per process). It is held in a type whose `Debug` and `Display` print `<redacted>` and is never placed in errors.

HTTP status mapping:
- 404 maps to `not_found`.
- 403 or 429 with rate-limit headers, and transport errors, map to `unavailable`.
- Anything else maps to `milkpan.fetch_failed` with `status` and `spec` details.

### Spec syntax and identity
- `github:owner/repo[@ref]`
- `github:owner/repo:path[@ref]`
- `github-release:owner/repo@tag#asset`

Canonicalization lowercases the owner and repository names (GitHub treats them case-insensitively) and keeps paths, refs and asset names as written. The canonical string is the identity and the authorization resource. The provider prefix (`github`) and the kind (`snapshot`, `file`, `release`) are passed as context facts `source.provider` and `source.kind`, so policies can condition on them.

### Authorization, configuration and lease settings
- **Configuration file:** `milkpan.toml` holds only panned `StaticRules` `[[rule]]` tables. Lease settings are `lease` obligations on those rules:
  ```toml
  [[rule]]
  scope = "ci"
  effect = "allow"
  actions = ["acquire", "reacquire", "read", "release"]
  resources = ["github:octo/*"]
  obligations = [{ name = "lease", params = { ttl = "1d", refresh = "10m", retain = "1" } }]
  ```
- **Default policy:** with no rules configured, milkpan uses the built-in rule: allow `acquire`, `reacquire`, `read` and `release` on `*` in scope `default`, with no obligations. Configuring any rule replaces the built-in rule entirely, so an explicit policy is never silently widened.
- **Lease hook:** a built-in after-fetch hook handles `lease`. It merges every `lease` obligation in the decision key by key, fails with `milkpan.conflicting_obligation` when a key has two different values, fills unset keys with defaults (refresh 1h, ttl 7d, retain 0) and writes the result onto the lease. There is no precedence between rules: two rules disagreeing is a configuration error to fix, not something to resolve silently.
- **Settings on the lease:** the applied `ttl`, `refresh` and `retain` are stored on each lease. Every acquire and renewal authorizes again (local, no network), so changed rules take effect at the next operation. `reacquire --due` and reclamation read the stored values.
- **Why obligations:** one policy file then explains both access and lifetime, settings can differ per resource, and Saucepan's own obligations use the same hook path. Alternative: a separate `[scope.<name>]` settings table. Rejected because it splits one scope's behaviour across two models.
- **Scope list:** the scopes are those named in rules, plus `default` when no rules are configured.
- **Principal:** taken from `--principal`, then `MILKPAN_PRINCIPAL`, then `user:<login name>`.
- **Swapping the authorizer:** the `Bucket` holds `Arc<dyn Authorizer>` and `Arc<dyn ScopeList>`, which is how Saucepan swaps in its engine.

### Extension surface
- `trait Provider { fn prefixes(&self) -> Vec<String>; fn canonicalize(&self, Spec) -> Result<Spec>; fn kind(&self, &Spec) -> String; fn resolve(&self, &Spec) -> Result<Resolved>; fn fetch(&self, &Spec, &Resolved, &mut dyn Write) -> Result<()> }`. `kind` is the `source.kind` policy fact; `Resolved` carries the version and a `ContentKind` (`File`, or `Archive` extracted into a tree).
- `trait AfterFetch { fn handles(&self, obligation: &str) -> bool; fn run(&self, &Stored, &[Obligation], Option<&mut LeaseSettings>) -> Result<()> }`. A hook receives every obligation it handles at once, which is what lets the `lease` hook merge them; it may set the lease's settings before the lease is recorded (`None` for reads, which record no lease). The built-in hooks are `verify` (re-hash the blob) and `lease` (settings).
- `trait BeforeReclaim { fn allow(&self, &BlobInfo) -> Result<bool> }`.
- `BucketBuilder` registers providers and hooks and sets the authorizer, home and clock. It registers no provider by itself: the `milkpan` binary adds `GitHub` exactly as a depending crate adds its own.
- `BucketCommands` implements panned's `CommandSet` over a `BucketHandle`, which builds the bucket on first use so `protocol` and argument errors need no home. The home is pre-scanned from `--home` before parsing, because the generic commands need the policy.

A `Clock` trait (system clock by default) makes cadence, time-to-live and expiry testable without sleeping.

### Testing approach
- **Loop:** tests use an in-memory fake provider that counts resolves and fetches.
- **GitHub provider:** tests run against a mock server (`tiny_http`, a dev-dependency) through `MILKPAN_GITHUB_API`, covering each kind, error mapping and token redaction.
- **CLI:** integration tests run the binary with a temporary `MILKPAN_HOME` and the mock server.
- **Live GitHub:** one `#[ignore]` test hits a real public repository and is run manually.

## Risks / Trade-offs

- [Path dependency on panned breaks clones and CI] → CI checks out panned beside milkpan. Switch to a git or crates.io dependency once panned is published.
- [Large repositories still download the whole tree] → File specs cover the read-a-few-files case. Snapshots carry no history, and cadence avoids repeat downloads.
- [Anonymous rate limit of 60 requests per hour] → The default 1-hour cadence makes repeat acquisitions offline. `unavailable` tells callers to retry or authenticate.
- [A release asset replaced under the same name gets a new id] → The asset id is the version, so reacquisition sees the change.
- [Lock wait blocks a CLI for up to 60 s] → Only state transitions hold the lock, and they take milliseconds. Downloads run outside it.
- [Windows-invalid file names in snapshots] → Extraction fails with `milkpan.fetch_failed` naming the entry, rather than writing a partial tree.
