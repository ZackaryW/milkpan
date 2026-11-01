# bucket-cli Specification

## Purpose

Expose the bucket as a command-line tool on panned's shared protocol, so scripts and SDKs drive milkpan, and later Saucepan, with the same envelope, error codes and exit codes.

## Requirements

### Requirement: Bucket commands
The `milkpan` binary SHALL provide `acquire <spec>`, `reacquire` (one lease or `--due`), `lease list`, `lease show <id>`, `release`, `path <lease> [file]` and `reclaim`. Lease commands SHALL take `--scope` (default `default`) and `--principal` (default from the environment or the local user).

#### Scenario: Acquire from the shell
- **WHEN** a user runs `milkpan --format json acquire github:octo/tools@v1.0`
- **THEN** stdout holds one envelope whose result names the lease id, version, digest and local path

#### Scenario: Path inside a snapshot
- **WHEN** a user runs `milkpan path <lease> docs/guide.md`
- **THEN** it prints the local path of that file within the leased snapshot, after authorizing `read`

### Requirement: The binary speaks panned's protocol
All commands SHALL use panned's envelope, base and namespaced error codes, exit codes and `--protocol` negotiation, and the binary SHALL include panned's generic `scope list` and `explain` commands over milkpan's authorizer and configured scopes.

#### Scenario: Denied in JSON mode
- **WHEN** an acquisition is denied with `--format json`
- **THEN** stdout is empty, stderr holds an envelope with code `denied` and evidence, and the exit code is 1

#### Scenario: Generic commands
- **WHEN** a user runs `milkpan scope list`
- **THEN** it lists the scopes defined in the configuration, or `default` without one

### Requirement: Bucket location is configurable
The bucket's state, blobs and configuration SHALL live under one home directory, chosen by `--home`, then `MILKPAN_HOME`, then `.milkpan` in the user's home directory. A bucket in another location SHALL NOT be read or migrated implicitly.

#### Scenario: Isolated bucket
- **WHEN** a command runs with `MILKPAN_HOME` set to an empty directory
- **THEN** it reads and writes only inside that directory

#### Scenario: Default home
- **WHEN** a command runs with neither `--home` nor `MILKPAN_HOME`
- **THEN** it uses `~/.milkpan`, creating it on first write

### Requirement: Commands are reusable by depending binaries
The bucket commands SHALL be available as a command set that a depending binary can register next to its own commands and providers.

#### Scenario: Saucepan reuses the commands
- **WHEN** a depending binary registers milkpan's command set with an extra provider
- **THEN** `acquire` in that binary accepts the extra provider's specs and uses the same envelope

### Requirement: Link commands
The binary SHALL provide `link <lease> --name <name> [--file <path>]`, `unlink <name>` and `link list`, and SHALL register panned's generic `slot list`, `slot show` and `slot switch` over milkpan's links.

#### Scenario: Self-management
- **WHEN** a user acquires milkpan's own release asset, runs `milkpan link <lease> --name milkpan --file <binary>` and later `milkpan reacquire --due`
- **THEN** `<home>/bin/milkpan` runs the new release on its next start

#### Scenario: Slot commands
- **WHEN** a user runs `milkpan slot list` with one link
- **THEN** the result lists that link with its current and switchable versions
