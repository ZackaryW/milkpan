# Tasks

## 1. Panned dependency

- [x] 1.1 Build against panned's `slot-contract` through the local `--config patch` override until it is on panned's GitHub `main`, then run `cargo update -p panned`; verify `cargo test --locked` passes with the GitHub pin and `Cargo.lock` names the slot-contract commit

## 2. Archive assets and executable bits

- [x] 2.1 Mark `*.tar.gz`, `*.tgz` and `*.zip` release assets as archives in the GitHub provider; detect tar.gz versus zip by magic bytes in `Store::tree` (new `zip` dependency); strip a top-level directory only when shared by every entry; verify mock-server and store tests for a zipped binary, a root-level archive and a plain binary asset staying a file
- [x] 2.2 Preserve executable bits on Unix when extracting tar and zip entries; verify a `cfg(unix)` test that an executable archive entry is executable after extraction

## 3. Links, shims and lifetime

- [x] 3.1 Add `links` to state (serde default) and `Bucket::link`, `unlink` and `links`: name validation, `conflict` on a taken name, `link` authorization, `files/<digest>/<filename>` views for file content, and atomic or near-atomic repointing (symlink rename on Unix, junction on Windows through the `junction` crate); verify tests for linking archive and file content, a taken name and repointing after `reacquire`
- [x] 3.2 Write shims for `--file` (relative symlink on Unix; `.cmd` plus a POSIX script on Windows) after checking the file exists; verify a test that runs the shim (through `cmd /C` on Windows) and gets the program's output and exit code, and that a missing file fails with `not_found` and creates nothing
- [x] 3.3 Keep linked leases from expiring, renew on `link`, and remove links and shims on `release` and `unlink`; verify tests for a linked lease past its time-to-live surviving reclamation and a release removing its link and shim

## 4. Reclamation in use

- [x] 4.1 Report content the platform refuses to delete as `in_use` in `Reclaimed` and continue; verify a `cfg(windows)` test that holds a file open inside an old tree, reclaims, sees the digest listed as in use and other content deleted, then reclaims again after closing it

## 5. Slots, CLI and documentation

- [x] 5.1 Implement panned's `Slot` for links (`LinkSlots`: current and retained available versions, `switch` repointing lease and links and reporting `next_run`) and add `link`, `unlink` and `link list` commands plus `GenericCommands::with_slots` in the binary; verify tests for rollback to a retained version, a reclaimed version failing with `invalid_request`, and `slot list` from the binary
- [x] 5.2 Add an integration test of the self-management flow: a mock release with an archive asset containing a script, acquire, link with `--file`, run the shim, publish a new release, `reacquire --due`, run the shim again and see the new output; document links, shims, rollback and self-management in the README; verify the README configuration test and `cargo doc --no-deps` with `-D warnings` pass

## Workflow follow-up

- Archive the change after verification, syncing `links-and-shims` and the additions to `github-provider`, `acquisition-loop` and `bucket-cli`.
