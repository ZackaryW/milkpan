# Spec Delta

## Purpose

Snapshot local git repositories at exact commits through the same acquisition loop as GitHub, so tests and local workflows need nothing but milkpan.

## ADDED Requirements

### Requirement: Local specs name a repository and a ref
The provider SHALL accept `local:<absolute path>[@ref]`, where the path is a git repository (working tree or bare) and a missing ref means `HEAD`. The canonical form SHALL use the absolute path with `/` separators. Local specs SHALL NOT take an inner path or `#asset`. A relative path SHALL fail with `invalid_request`.

#### Scenario: Windows path
- **WHEN** the spec `local:C:\work\tools@main` is canonicalized
- **THEN** its canonical form is `local:C:/work/tools@main`

#### Scenario: Relative path
- **WHEN** the spec `local:tools@main` is given
- **THEN** it fails with `invalid_request` before running git

### Requirement: Local snapshots contain committed content only
Resolving SHALL produce the full commit SHA the ref points at. Fetching SHALL produce the repository's tracked files at that commit, without the `.git` directory, uncommitted changes or untracked files.

#### Scenario: Uncommitted edits
- **WHEN** a local repository has an uncommitted change and its `HEAD` is acquired
- **THEN** the snapshot holds the committed content, not the change

#### Scenario: Ref moved
- **WHEN** a new commit is made on the acquired branch and the lease is reacquired
- **THEN** the lease swaps to the new commit's snapshot

### Requirement: Local failures map to stable codes
A path that is not a git repository, or a ref that does not resolve to a commit, SHALL fail with `not_found` naming the spec. A missing `git` command SHALL fail with `unavailable`.

#### Scenario: Unknown ref
- **WHEN** `local:/repo@no-such-branch` is acquired
- **THEN** it fails with `not_found` and details naming the spec
