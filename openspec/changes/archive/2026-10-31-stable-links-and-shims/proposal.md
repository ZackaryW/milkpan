# Proposal

## Why

Milkpan can acquire and reacquire release binaries, including its own, but they live at content-addressed paths that change with every version, so nothing can run them from a stable place and nothing can update them. Following Scoop, a stable link that follows the lease, plus a launcher on PATH, makes updates take effect on the next run without replacing a running program. With that, milkpan manages itself (and any released tool) through the same acquire and reacquire loop.

## What Changes

- **Archive release assets**: `.tar.gz`, `.tgz` and `.zip` release assets are extracted into trees like snapshots, so a binary inside has a stable relative path.
- **Links**: `milkpan link <lease> --name <name>` keeps `<home>/links/<name>` pointing at the lease's current content. Every swap retargets it; running programs keep the version they started from.
- **Shims**: with `--file <path>`, `<home>/bin/<name>` launches that file through the link. Users put `<home>/bin` on PATH once.
- **Linked leases do not expire** while linked, and releasing a lease removes its links and shims.
- **Reclaim tolerates content in use**: an old version that cannot be deleted (a running program on Windows) is skipped and retried on the next reclamation.
- **Panned slots**: links implement panned's `Slot` contract, so `slot list|show|switch` work and `slot switch` rolls a link back to a retained version.

## Capabilities

### New Capabilities
- `links-and-shims`: stable links that follow leases, launcher shims, linked-lease lifetime and slot switching.

### Modified Capabilities
- `github-provider`: archive release assets are extracted into trees.
- `acquisition-loop`: reclamation skips content in use.
- `bucket-cli`: `link`, `unlink` and `link list` commands, and panned's slot commands.

## Impact

- New `links` module, `link` action in the built-in policy, zip extraction (new `zip` dependency) and Windows junction support (new `junction` dependency, Windows only).
- **Depends on**: panned's `slot-contract` change.
- **Out of scope**: editing PATH, Windows launcher executables (shims are `.cmd` plus a POSIX script there), and service or process management.
