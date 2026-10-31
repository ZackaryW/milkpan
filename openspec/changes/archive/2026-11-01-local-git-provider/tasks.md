# Tasks

## 1. Selector-independent reuse

- [x] 1.1 Key `state.versions` by spec without selector plus version (`version_key`), keeping the asset; verify a bucket test where acquiring `@main` then `@<resolved version>` in another scope downloads once, and the existing retention and rollback tests still pass

## 2. Local provider

- [x] 2.1 Add `provider/local.rs` (`Local`, prefix `local`): canonicalization (Windows drive re-join, absolute path, `/` separators, no inner path or asset), `rev-parse` resolution and `git archive` fetch with stable error codes; register it in the binary; verify tests on a temporary repository for resolving `HEAD` and a branch, a snapshot excluding uncommitted and untracked files and `.git`, reacquisition after a new commit, an unknown ref (`not_found`), a relative path (`invalid_request`) and Windows-path canonicalization
- [x] 2.2 Document local specs in the README spec table and note the `git` requirement; verify the README test and `cargo doc --no-deps` with `-D warnings` pass

## Workflow follow-up

- Archive after verification, syncing `local-provider` and the `resource-model` addition; release a new milkpan version for Powerspec to pin.
