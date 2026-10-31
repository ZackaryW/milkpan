# Proposal

## Why

Consumers such as Powerspec need a dependable bucket of GitHub resources: fetch a repository, file or snapshot at a ref or commit, keep it cached, reacquire it when the ref moves, and authenticate with the existing `gh` login. The current Saucepan provides this only inside a large system of claims, groups, leases, pins and self-management, and re-downloads whole repositories to read a few files. Milkpan is a small Rust resource bucket for GitHub that does this one job well, built on panned's core, and it is the foundation the new Saucepan extends.

## What Changes

- **Four nouns**: a *spec* (what to fetch: source plus selector), a *version* (what the selector resolved to, such as a commit or release id), a *blob* (stored content, addressed by digest), and a *lease* (a scope holding one version of one spec until it expires or is renewed).
- **Core loop**: resolve the selector, fetch only versions not already stored, store content-addressed, lease the version to a scope. **Reacquire** resolves again; when the version changed it fetches the new one and swaps the lease atomically. Blobs with no lease are reclaimed.
- **GitHub provider** (built in): repository snapshots without history, single files at a commit, and release assets, each pinned to an exact commit or release; requests borrow the token from `gh auth token` when available and proceed anonymously otherwise. Tokens are never stored or printed.
- **Extension points**: a `Provider` trait (resolve and fetch), panned's `Authorizer` as the gate on acquire, renew and read (default: `StaticRules` from a small configuration file), and hooks after fetch (to perform obligations such as verification) and before reclaim. Additional providers are added by depending crates.
- **Panned-native model**: scopes and principals are panned's core types and every lease operation is a panned authorization request, so a stronger authorizer (such as Saucepan's engine, whose grants map onto leases) can replace the default without code changes in milkpan.
- **Lease settings as obligations**: refresh cadence, lease time-to-live and retained versions are parameters of a `lease` obligation in the same policy that grants access, applied by a built-in hook, so one policy file explains a scope and settings can differ per resource.
- **CLI** built on panned's CLI contract (shared JSON envelope, error codes, exit codes, protocol version) with bucket commands such as `acquire`, `reacquire`, `lease` and `release`, so SDK consumers can target milkpan or Saucepan with the same protocol.

## Capabilities

### New Capabilities
- `resource-model`: specs, versions, content-addressed blobs and leases, and their invariants.
- `acquisition-loop`: resolve, fetch-if-new, store, lease, reacquire with atomic swap, and reclaim of unleased blobs.
- `github-provider`: repository snapshots, single files and release assets at exact versions, with borrowed `gh` credentials.
- `extension-points`: the `Provider` trait, panned authorizer integration, and fetch and reclaim hooks.
- `lease-settings`: refresh cadence, lease time-to-live and retained versions from `lease` obligations, with defaults and a conflict rule.
- `bucket-cli`: bucket commands on panned's CLI contract.

### Modified Capabilities
<!-- None: new project. -->

## Impact

- **New crate and binary** `milkpan` (Rust), depending on panned's core and CLI contract only (no engine).
- **Downstream**: the new Saucepan depends on milkpan and adds providers and panned's engine; Powerspec can use milkpan directly through an SDK speaking the shared protocol.
- **Out of scope**: non-GitHub providers, multi-tenant handshakes, process-bound leases, self-installation and release management (Saucepan concerns).
- **Depends on**: panned's `establish-core-and-cli-contract` change for the core types and CLI contract, and panned's `static-rules-obligations` change so core rules can carry obligations.
