# github-provider Specification

## Purpose

Fetch GitHub repository snapshots, single files and release assets at exact versions, transferring no more than the selected content and borrowing the user's existing GitHub credentials.

## Requirements

### Requirement: Three GitHub spec kinds
The provider SHALL accept repository snapshots (`github:owner/repo@ref`), single files (`github:owner/repo:path/to/file@ref`) and release assets (`github-release:owner/repo@tag#asset`). A missing `@ref` SHALL mean the default branch; the release tag `latest` SHALL mean the latest published release; an asset name MAY contain `*` and SHALL match exactly one asset.

#### Scenario: Default branch
- **WHEN** a spec `github:octo/tools` is resolved
- **THEN** the version is the commit at the head of the repository's default branch

#### Scenario: Ambiguous asset pattern
- **WHEN** the asset pattern `tools-*` matches two assets of the release
- **THEN** resolution fails with `invalid_request` naming the matching assets

### Requirement: Snapshots carry no history
A repository snapshot SHALL contain the repository's files at the resolved commit and no version-control history. The local path of a snapshot SHALL be a directory tree of those files.

#### Scenario: Snapshot contents
- **WHEN** a snapshot spec is acquired
- **THEN** its local path is a directory holding the files at that commit, without a `.git` directory

### Requirement: Single files transfer only the file
A single-file spec SHALL download only that file's content at the resolved commit, not the repository.

#### Scenario: Reading one file
- **WHEN** a single-file spec for `README.md` is acquired
- **THEN** only `README.md` is downloaded and its local path is that file

### Requirement: Credentials are borrowed, never kept
Requests SHALL authenticate with a token from `GH_TOKEN`, then `GITHUB_TOKEN`, then `gh auth token`, and SHALL proceed anonymously when none is available. Tokens SHALL never be written to disk, logs, command output or error details.

#### Scenario: No gh login
- **WHEN** no token is available and a public repository is acquired
- **THEN** the request is made anonymously and succeeds

#### Scenario: Error output
- **WHEN** an authenticated request fails
- **THEN** the error reports the status and repository but not the token

### Requirement: Provider failures map to stable codes
A source or ref that does not exist SHALL fail with `not_found`; a rate limit or network failure SHALL fail with `unavailable`; other provider failures SHALL fail with `milkpan.fetch_failed`. Details SHALL include the spec and, when known, the HTTP status.

#### Scenario: Unknown repository
- **WHEN** a spec names a repository that does not exist or is not visible
- **THEN** the command fails with `not_found` and details naming the spec

### Requirement: Archive release assets are extracted
A release asset whose name ends in `.tar.gz`, `.tgz` or `.zip` SHALL be stored as an archive and presented as a directory tree, with a single top-level directory stripped when every entry shares it. Other assets SHALL remain single files.

#### Scenario: Zipped binary
- **WHEN** the asset `tool-windows.zip` containing `tool-1.0/tool.exe` is acquired
- **THEN** the lease's path is a directory containing `tool.exe`

#### Scenario: Plain binary asset
- **WHEN** the asset `tool-linux` is acquired
- **THEN** the lease's path is that file
