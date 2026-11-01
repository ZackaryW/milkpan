# Tasks

## 1. Default home

- [x] 1.1 Change `cli::resolve_home` to fall back to `.milkpan` in the user home (`std::env::home_dir`, which honors `HOME`/`USERPROFILE` on every platform) (keeping `--home` and `MILKPAN_HOME` first) and update its rustdoc; verify unit tests for each precedence level with a fake home directory and an integration test that runs the binary with `HOME`/`USERPROFILE` pointed at a temporary directory and finds the bucket in `.milkpan`
- [x] 1.2 Update the README (configuration and bucket layout sections, plus moving an old data-directory bucket or pointing `MILKPAN_HOME` at it) and bump the version to 0.3.0; verify the README test and `cargo test --locked` pass

## Workflow follow-up

- Archive after verification, syncing the `bucket-cli` modification; tag v0.3.0 so Powerspec can pin it.
