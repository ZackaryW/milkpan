# Proposal

## Why

Powerspec is moving from a private bucket (`~/.powerspec/milkpan`) to one bucket shared with plain `milkpan` use, so a commit fetched by either is stored once and one reset clears everything. Sharing only works if every tool resolves the same home without environment variables. Today milkpan defaults to the per-user platform data directory (`%LOCALAPPDATA%\milkpan`, `~/.local/share/milkpan`), which is hard to find and to wipe. A visible `~/.milkpan` suits a shared, resettable bucket.

## What Changes

- **BREAKING:** the default bucket home becomes `~/.milkpan` (the user's home directory). `--home` and `MILKPAN_HOME` still take precedence.
- **No migration:** a bucket left in the old data directory is not migrated or read. The README says how to move it, or how to point `MILKPAN_HOME` at it.
- **Release:** milkpan 0.3.0, the first release with the new default. Powerspec pins it next.

## Capabilities

### New Capabilities
<!-- None. -->

### Modified Capabilities
- `bucket-cli`: the default home is `~/.milkpan`.

## Impact

- `src/cli.rs` (`resolve_home`), the README and the version bump to 0.3.0.
- Library users calling `cli::resolve_home` get the new default.
- Unblocks Powerspec's `shared-milkpan-and-force-reset`.
