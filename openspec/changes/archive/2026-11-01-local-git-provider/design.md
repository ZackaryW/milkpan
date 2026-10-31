# Design

## Context

See proposal.md. Providers implement `prefixes`, `canonicalize`, `kind`, `resolve` and `fetch`. `Spec::parse` splits `prefix:source[:path][@selector][#asset]` on the first `:` after the prefix, so a Windows path (`C:/x`) parses as source `C` and path `/x`. The store extracts gzip tar archives into trees and strips a top-level directory shared by every entry. `state.versions` maps `"<canonical spec> <version>"` to a digest.

## Goals / Non-Goals

**Goals:**
- Powerspec's integration tests can use local repositories through milkpan.
- Committed content only, so a local snapshot is as reproducible as a GitHub one.

**Non-Goals:**
- Remote non-GitHub git hosts (clone/fetch over the network): left to Saucepan's providers.
- Working-tree snapshots including uncommitted changes.

## Decisions

### Shell out to `git`
`git -C <path> rev-parse --verify --end-of-options <ref>^{commit}` resolves the ref. `git -C <path> archive --format=tar.gz --prefix=snapshot/ <sha>` streams the snapshot to the store, and the shared `snapshot/` directory is stripped on extraction. Both run with `GIT_TERMINAL_PROMPT=0` and no shell. The alternative, the `gix` crate, would add a large dependency to serve tests and local work. Users of local specs already have git.

- A `git` that cannot be started maps to `unavailable`.
- A non-zero exit from `rev-parse` maps to `not_found`, with git's first stderr line in details.
- A non-zero exit from `archive` maps to `milkpan.fetch_failed`.

### Windows paths in the shared grammar
`canonicalize` for `local` re-joins a one-letter source with the parsed inner path (`C` and `/x` become `C:/x`). It then requires an absolute path, replaces `\` with `/` and drops a trailing `/`. The result is idempotent: the canonical string parses back to the same split and canonicalizes to itself. Any other inner path or `#asset` is rejected. The path is taken as given, not resolved through symlinks, so the identity is what the user wrote, made absolute.

### Selector-independent version index
`version_key` becomes `"<spec without selector> <version>"`, with the asset kept, because it chooses different content. A commit reached through `@main`, `@v1` or `@<sha>` maps to one digest. Retention looks up history versions through the same key, so rollback is unaffected. Old entries keyed by full spec are simply not found and are re-fetched once. Reclamation prunes index entries whose blob is gone, so stale keys disappear naturally.

## Risks / Trade-offs

- [`git` missing on a machine] → Only local specs need it, and the error says so (`unavailable`).
- [Large local repositories produce large archives] → Same trade-off as GitHub snapshots; acquiring within the refresh cadence avoids repeats.
