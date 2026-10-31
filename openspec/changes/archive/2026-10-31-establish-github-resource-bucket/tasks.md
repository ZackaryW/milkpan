# Tasks

## 1. Crate setup

- [x] 1.1 Confirm panned's `static-rules-obligations` change is implemented (rules with obligations deserialize from `[[rule]]` tables and appear in allow decisions); verify by a milkpan test that builds `StaticRules` with an obligation and reads it back from a decision
- [x] 1.2 Create the `milkpan` crate (Rust 2024, MIT, library plus `milkpan` binary) depending on `panned` by path (`../panned`, default features), with rustfmt, clippy and a CI workflow that checks out panned beside milkpan and runs fmt, clippy `-D warnings` and tests; verify `cargo build` and `cargo test` succeed locally

## 2. Resource model and store

- [x] 2.1 Implement spec parsing and canonicalization for `github:owner/repo[@ref]`, `github:owner/repo:path[@ref]` and `github-release:owner/repo@tag#asset`, with prefix, source, selector and kind; verify unit tests for canonical identity, case folding and malformed specs failing with `invalid_request`
- [x] 2.2 Implement the blob store (streamed SHA-256 into `tmp/`, rename into `blobs/sha256/`, reuse of existing digests) and safe tarball extraction into `trees/` (strip top-level directory, reject absolute, `..` and escaping links); verify unit tests for shared content, immutability and rejected archive entries
- [x] 2.3 Implement the state file (leases with id, spec, scope, principal, version, digest, resolved and expiry times; version history; version-to-digest index) with the exclusive lock and atomic replace, and a `Clock` trait; verify a test where two threads record different leases concurrently and both survive, and one where an interrupted write leaves the previous state

## 3. Acquisition loop, lease settings and extension points

- [x] 3.1 Implement configuration (`milkpan.toml` `[[rule]]` StaticRules with obligations, built-in `default` policy replaced by any configured rule); verify tests for the no-file first run and a broken file failing with `invalid_request` naming it
- [x] 3.2 Implement the `Provider` trait and registry, `AfterFetch` and `BeforeReclaim` hooks, the built-in `verify` hook, the built-in `lease` hook (duration parsing, key-by-key merge, `milkpan.conflicting_obligation`, defaults) and `BucketBuilder`; verify tests that an unknown prefix lists the registered prefixes, an unhandled obligation fails with `milkpan.unmet_obligation` without a lease, and every lease-settings scenario for merge, conflict, invalid values and defaults
- [x] 3.3 Implement `acquire`, `reacquire`, `reacquire --due`, `release` and `read` (authorized before provider access or state change, with `source.provider` and `source.kind` context) and reclamation with retained versions and reclaim vetoes; verify fake-provider tests for every acquisition-loop, extension-points and lease-settings scenario (including per-resource settings and retained versions), including offline renewal within cadence, failed fetch keeping the old version, partial `--due` refresh reported as non-complete, and denied acquisition reaching no provider

## 4. GitHub provider

- [x] 4.1 Implement token borrowing (`GH_TOKEN`, `GITHUB_TOKEN`, `gh auth token`, anonymous fallback) in a redacting type, and HTTP status mapping to `not_found`, `unavailable` and `milkpan.fetch_failed`; verify unit tests that Debug, Display and error details never contain the token
- [x] 4.2 Implement snapshot, single-file and release-asset resolve and fetch against `MILKPAN_GITHUB_API`; verify mock-server tests (`tiny_http`) for default branch resolution, a single file transferring only that file, a snapshot without `.git`, `latest` releases, an ambiguous asset pattern and an unknown repository, plus one `#[ignore]` live test against a public repository

## 5. CLI and documentation

- [x] 5.1 Implement `BucketCommands` (`acquire`, `reacquire`, `lease list`, `lease show`, `release`, `path`, `reclaim`, with `--scope`, `--principal` and `--home`/`MILKPAN_HOME`) and the binary assembling panned's `Runner`, `GenericCommands` and `BucketCommands`; verify integration tests running the binary with a temporary home and the mock server for acquire, path inside a snapshot, denial in JSON mode, `scope list` and a depending command set registering an extra provider
- [x] 5.2 Write the README (spec syntax, commands, configuration file, credentials, bucket layout, extension points) and rustdoc for public items; verify `cargo doc --no-deps` has no warnings and the README's configuration example is parsed by a test

## Workflow follow-up

- Verify the change, then archive it, synchronizing the six capability specs.
