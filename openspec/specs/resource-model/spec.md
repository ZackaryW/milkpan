# resource-model Specification

## Purpose

Define the four nouns of the bucket (specs, versions, blobs and leases) and the invariants that keep stored content addressable, shareable and safe to reclaim.

## Requirements

### Requirement: A spec names a source and a selector
A spec SHALL identify what to fetch as a provider, a source within that provider and a selector, and SHALL have one canonical string form that is its identity. Two specs with the same canonical form SHALL be the same spec. The canonical form SHALL also be the resource name used in authorization requests.

#### Scenario: Canonical identity
- **WHEN** a spec is given as `github:octo/tools@main` and again with the same parts in another accepted spelling
- **THEN** both resolve to one canonical form and share leases and versions

#### Scenario: Malformed spec
- **WHEN** a spec string names no provider or no source
- **THEN** it is rejected with `invalid_request` before any network access

### Requirement: A version is an exact resolution
Resolving a spec's selector SHALL produce a version that identifies immutable content, such as a commit SHA or a release asset id. A version SHALL never be a moving name such as a branch.

#### Scenario: Branch resolves to a commit
- **WHEN** the selector `main` is resolved
- **THEN** the version recorded is the full commit SHA that `main` pointed to at that moment

### Requirement: Blobs are content-addressed
Stored content SHALL be addressed by the SHA-256 digest of its bytes. Fetching a version whose content is already stored SHALL reuse the existing blob, and a blob SHALL never be modified after it is written.

#### Scenario: Shared content
- **WHEN** two specs resolve to versions with identical bytes
- **THEN** the bucket stores one blob and both versions refer to it

### Requirement: A lease holds one version for one scope
A lease SHALL bind one version of one spec to one scope and principal, with an expiry time. A scope SHALL hold at most one lease per spec. A lease's version SHALL stay readable until the lease is released, expires or is swapped by reacquisition.

#### Scenario: One lease per spec and scope
- **WHEN** scope `ci` acquires the same spec twice
- **THEN** it holds one lease, whose expiry was extended by the second acquisition

#### Scenario: Same spec in two scopes
- **WHEN** scopes `ci` and `dev` acquire the same spec
- **THEN** each holds its own lease, and both may point to the same version
