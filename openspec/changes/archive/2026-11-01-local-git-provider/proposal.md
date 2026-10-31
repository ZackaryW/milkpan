# Proposal

## Why

Powerspec is switching its source store from the old Saucepan to milkpan. Production sources are GitHub repositories, but Powerspec's integration tests (and local profile development) use local git repositories, which milkpan cannot fetch. A built-in local provider lets those tests and workflows run through milkpan alone, with no second code path in Powerspec.

Powerspec also selects sources by exact commit: it resolves a ref, then holds a lease on `@<commit>`. Milkpan currently indexes stored versions per full spec, so the same commit under `@main` and `@<sha>` is downloaded twice.

## What Changes

- **Local provider**: `local:<absolute path>[@ref]` snapshots a local git repository at a commit. The ref defaults to `HEAD`. It resolves with `git rev-parse` and fetches with `git archive`, so only committed content is used, never the working tree.
- **Selector-independent reuse**: stored versions are indexed by spec without its selector, so one commit reached through different selectors (`@main`, `@v1`, `@<sha>`) is fetched once.

## Capabilities

### New Capabilities
- `local-provider`: local git repositories as snapshot sources through the `git` command.

### Modified Capabilities
- `resource-model`: a version's stored content is shared by every selector of the same source.

## Impact

- New `provider/local.rs`, registered by the `milkpan` binary next to GitHub. It needs `git` on PATH only when `local:` specs are used.
- `state.versions` keys change from `"<spec> <version>"` to `"<spec without selector> <version>"`. Entries recorded under old keys are missed once and fetched again; nothing breaks.
- Unblocks Powerspec's `switch-sources-to-milkpan`.
