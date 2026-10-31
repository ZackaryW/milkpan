# Design

## Context

See proposal.md. Milkpan stores content immutably (`blobs/`, `trees/<digest>/`), swaps leases only after new content is stored, and keeps retained versions from reclamation. Release assets are stored as single files today. Archive extraction (`Store::tree`) writes plain files and drops the executable bit. Panned's `slot-contract` change adds `Slot`, `SlotState`, `Switch`, `check_switch` and `GenericCommands::with_slots`.

## Goals / Non-Goals

**Goals:**
- Updates that take effect on the next run, on Windows as well as Unix, with no special self-update code: milkpan updates itself through acquire, link and reacquire.
- Links that are always consistent with the lease they follow.

**Non-Goals:**
- Editing PATH or shell profiles; the user adds `<home>/bin` once.
- A compiled Windows launcher executable; `.cmd` plus a POSIX script cover cmd, PowerShell and Git Bash for now.
- Detecting running processes; "in use" is whatever the platform refuses to delete.

## Decisions

### Archive release assets
The GitHub provider sets `ContentKind::Archive` for assets named `*.tar.gz`, `*.tgz` or `*.zip`. `Store::tree` detects the format by magic bytes (`1f 8b` gzip tar, `50 4b 03 04` zip) instead of trusting a flag, and strips a top-level directory only when every entry shares it; a release archive with files at its root is kept as is. Snapshots keep today's behaviour, since GitHub tarballs always have one top directory.

Extraction now preserves the executable bit on Unix (tar header mode, zip `unix_mode`), masked to `0o755`/`0o644`. Without this, a released binary extracted from an archive could not run.

### Links
`<home>/links/<name>` points at the content directory of the linked lease:
- **Archive content:** `trees/<digest>/`.
- **File content (plain assets, single files):** `files/<digest>/<filename>`, created when first linked. The file name is the asset name or the last component of the spec path. A hard link to the blob is used, with a copy as fallback, and the executable bit is set when a shim targets it.

Repointing is done under the state lock:
- **Unix:** create a symlink at `links/.<name>.<uuid>` and `rename` it over `links/<name>`, which is atomic.
- **Windows:** create a directory junction (no privilege needed, unlike symlinks) at a temporary name, remove the old junction and rename the new one into place. Windows cannot rename over an existing directory, so a start that races the repoint can briefly miss the link. The window is a few system calls and is accepted.

Removing a junction or symlink never touches its target, so a running program's tree is untouched.

State gains `links: name -> { lease, file }` (serde default, so existing state files load). After `refresh` swaps a lease's version, every link to that lease is repointed inside the same transaction that records the swap.

### Shims
`--file <path>` validates that the file exists in the content, then writes:
- **Unix:** a relative symlink `bin/<name> -> ../links/<name>/<path>`. `exec` through a symlink keeps arguments, streams and the exit code.
- **Windows:**
  - `bin/<name>.cmd` with `@"%~dp0..\links\<name>\<path>" %*`, which returns the program's exit code;
  - an extensionless `bin/<name>` POSIX script that `exec`s the same target, for Git Bash and MSYS shells.

Shims point through the link, never at a tree, so they never need rewriting after an update.

### Lifetime
Reclamation skips expiry for leases that have links. `release` removes the lease's links and shims before removing the lease. `unlink` removes one link (and its shim); once a lease has no links, its normal `expires_at` applies again. `link` renews the lease's expiry like an acquisition.

### In-use tolerance
`Store::remove` reports "in use" separately from other failures: `PermissionDenied`, or raw OS errors 5 and 32 on Windows. Reclamation records those digests in `Reclaimed::in_use`, keeps their versions-index entries, and continues. Other errors still fail.

### Slots
`LinkSlots` implements panned's `Slot` over a `Bucket`:
- **`slots` and `slot`:** report each link's lease version as `current`. `available` lists the current version plus the retained history versions whose blobs are still stored.
- **`switch`:** uses `check_switch`, authorizes `link` for the default principal, then sets the lease's `version` and `digest` to the retained version and repoints its links, all under the lock. It reports `next_run`.
- **Selector and resolution time:** the selector is untouched and `resolved_at` is kept, so `reacquire --due` may move the lease forward again. Pinning means changing the spec's `@tag`.

The `Slot` trait carries no principal, so slot commands use the default principal (`MILKPAN_PRINCIPAL` or `user:<login>`). The bucket's own `link`/`unlink` commands take `--principal`.

The built-in policy's actions grow to include `link`.

### Self-management
Self-management uses no dedicated code; the README documents the flow:
```
milkpan acquire "github-release:ZackaryW/milkpan@latest#milkpan-*-<target>.zip" --scope self
milkpan link <lease> --name milkpan --file milkpan.exe      # or milkpan on Unix
milkpan reacquire --due                                     # next start runs the new release
```
On Windows, the running `milkpan.exe` lives in `trees/<old>/`. The junction is repointed, the old tree is reported in use by reclamation until that process exits, and it is deleted on a later reclamation.

### Panned dependency
`slot-contract` must be on panned's GitHub `main` before milkpan's lockfile can pin it. Until then, development uses the documented `--config patch` override, and the final step is `cargo update -p panned`.

## Risks / Trade-offs

- [The Windows junction repoint is not atomic] → It is done under the lock, the window is tiny, and a start that misses it can simply be retried.
- [`.cmd` shims show "Terminate batch job?" on Ctrl+C in cmd] → Documented. A launcher executable can replace the `.cmd` later without changing links.
- [Hard links fail across volumes] → Fall back to a copy. `files/` lives in the same home as `blobs/`, so this is rare.
- [Rollback is undone by the next due reacquisition] → Documented. Pinning a version means changing the selector, so the policy stays visible in the spec.
