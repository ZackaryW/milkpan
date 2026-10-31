# extension-points Specification

## Purpose

Let depending crates such as the new Saucepan add providers, replace the authorizer and act on fetched or reclaimed content, without changing milkpan.

## Requirements

### Requirement: Providers plug in by spec prefix
Milkpan SHALL expose a provider interface with two operations, resolving a spec to a version and fetching a version's content, and SHALL route each spec to the provider registered for its prefix. A spec whose prefix has no provider SHALL fail with `invalid_request` naming the registered prefixes.

#### Scenario: Added provider
- **WHEN** a depending crate registers a provider for prefix `gitea`
- **THEN** specs starting with `gitea:` are resolved and fetched by it through the same acquisition loop

#### Scenario: Unknown prefix
- **WHEN** a spec starts with a prefix no provider handles
- **THEN** it fails with `invalid_request` listing the known prefixes

### Requirement: Every lease operation is a panned authorization request
Acquire, reacquire, read and release SHALL each be authorized through panned's authorizer interface before any provider access or state change, with the principal, the scope, the operation as action and the canonical spec as resource. A denial SHALL fail with `denied` and the decision evidence, and SHALL change nothing.

#### Scenario: Denied acquisition
- **WHEN** the authorizer denies `acquire` on a spec in scope `ci`
- **THEN** no request reaches the provider, no lease is created and the error carries the evidence

#### Scenario: Replacement authorizer
- **WHEN** a depending crate supplies its own authorizer
- **THEN** milkpan uses it for every lease operation without other changes

### Requirement: Obligations are fulfilled by hooks or the operation fails
After content is stored and before the lease is recorded, milkpan SHALL pass the allow decision's obligations to its after-fetch hooks. Every obligation SHALL be fulfilled by some hook; an obligation no hook fulfils, or a hook failure, SHALL fail the operation with `milkpan.unmet_obligation` and leave leases unchanged. Milkpan SHALL provide a built-in `verify` hook that re-hashes stored content against its digest.

#### Scenario: Unknown obligation
- **WHEN** the authorizer allows with obligation `scan` and no hook handles it
- **THEN** acquisition fails with `milkpan.unmet_obligation` and no lease is created

#### Scenario: Built-in verification
- **WHEN** the authorizer allows with obligation `verify`
- **THEN** the stored content is re-hashed and the lease is recorded only if the digest matches

### Requirement: Reclaim hooks can veto deletion
Before deleting a blob, milkpan SHALL call its before-reclaim hooks; a hook that refuses SHALL keep that blob until the next reclamation, and the refusal SHALL be reported.

#### Scenario: Vetoed reclaim
- **WHEN** a before-reclaim hook refuses a blob
- **THEN** the blob is kept and the reclaim result lists it as retained by a hook
