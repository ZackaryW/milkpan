# Spec Delta

## Purpose

Expose a lease's content at a stable path that follows reacquisition, and launch files from it through shims on PATH, so updates, including milkpan's own, take effect on the next run without replacing a running program.

## ADDED Requirements

### Requirement: A link follows its lease
Linking a lease under a name SHALL make `<home>/links/<name>` resolve to the lease's current content directory. Whenever the lease's version changes, the link SHALL be repointed after the new content is stored. Link names SHALL be unique in a bucket and use letters, digits, `-`, `_` or `.`. Linking SHALL be authorized as action `link` on the lease's spec.

#### Scenario: Link survives an update
- **WHEN** a linked lease is reacquired to a new version
- **THEN** `<home>/links/<name>` resolves to the new version's content

#### Scenario: Name taken
- **WHEN** a link name already belongs to another lease
- **THEN** linking fails with `conflict` naming the existing lease

### Requirement: Running programs keep their version
Repointing a link SHALL NOT modify or delete the content it pointed at before, so a program started through the link keeps running the version it started with, and the next start uses the new version.

#### Scenario: Update while running
- **WHEN** a program started through link `tool` is running and the lease is reacquired to a new version
- **THEN** the running program is unaffected and the next start through `tool` runs the new version

### Requirement: Shims launch a file through a link
Linking with a file path SHALL create a launcher `<home>/bin/<name>` (with `.cmd` and an extensionless POSIX script on Windows) that runs that file inside the link, passing arguments, standard streams and the exit code through. The file SHALL exist in the lease's content when linking.

#### Scenario: Run from PATH
- **WHEN** `<home>/bin` is on PATH and `tool --version` is run
- **THEN** the linked file runs with `--version` and its exit code is returned

#### Scenario: Missing file
- **WHEN** linking names a file that is not in the lease's content
- **THEN** linking fails with `not_found` and nothing is created

### Requirement: Linked leases stay held
A lease SHALL NOT expire while it has links. Releasing a lease or unlinking its last link SHALL remove the links and shims, after which normal expiry applies.

#### Scenario: Long-lived tool
- **WHEN** a linked lease passes its time-to-live
- **THEN** reclamation keeps the lease, its content and its link

#### Scenario: Release removes links
- **WHEN** a linked lease is released
- **THEN** its link and shim are removed

### Requirement: Links are panned slots
Each link SHALL be a panned slot named after the link, whose current version is the lease's version and whose switchable versions are that version plus the retained previous versions. Switching a slot SHALL repoint the link and the lease to the chosen retained version, report effect `next_run`, and leave the spec's selector unchanged, so a later reacquisition may move it forward again.

#### Scenario: Roll back
- **WHEN** a link with one retained previous version is switched to that version
- **THEN** the link and lease point at it and the switch reports `next_run`

#### Scenario: Not retained
- **WHEN** a link is switched to a version that was reclaimed
- **THEN** the switch fails with `invalid_request` listing the switchable versions
