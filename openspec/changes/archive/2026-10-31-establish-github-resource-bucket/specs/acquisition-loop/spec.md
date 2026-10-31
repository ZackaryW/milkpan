# Spec Delta

## Purpose

Acquire, keep and refresh resources with the least network traffic: resolve, fetch only what is new, store it, lease it, swap leases atomically on reacquisition and reclaim what nothing holds.

## ADDED Requirements

### Requirement: Acquire resolves, fetches if new and leases
Acquiring a spec for a scope SHALL resolve its selector to a version, fetch content only when that version is not already stored, store it, and create or renew the scope's lease on that version. The result SHALL report the lease, the version, the blob digest and a local path to the content.

#### Scenario: First acquisition
- **WHEN** a scope acquires a spec whose version is not stored
- **THEN** the content is fetched once, stored and leased to the scope

#### Scenario: Already stored
- **WHEN** a scope acquires a spec whose resolved version is already stored
- **THEN** no content is downloaded and the lease points to the stored version

### Requirement: Acquire within the refresh cadence skips resolution
When a scope already holds a lease on a spec and the last resolution is newer than the lease's refresh cadence, acquiring SHALL renew the lease without contacting the provider.

#### Scenario: Offline renewal
- **WHEN** a scope whose lease has a 1-hour cadence acquires a spec it resolved 10 minutes ago and the network is unavailable
- **THEN** the acquisition succeeds with the leased version

### Requirement: Reacquire swaps leases atomically
Reacquiring a lease SHALL resolve the selector again regardless of cadence. When the version changed, the new content SHALL be fetched and stored before the lease is switched, so readers see either the old version or the new one, never a partial state. When the version is unchanged, only the resolution time and expiry SHALL be updated.

#### Scenario: Ref moved
- **WHEN** a branch moves to a new commit and the lease is reacquired
- **THEN** the lease points to the new commit only after its content is fully stored

#### Scenario: Fetch fails during reacquisition
- **WHEN** fetching the new version fails
- **THEN** the lease keeps its previous version and the command fails with the provider error

### Requirement: Due leases can be reacquired in one call
The bucket SHALL reacquire, in one operation, every lease whose refresh cadence has elapsed. When some reacquisitions succeed and others fail, the operation SHALL report each outcome and finish as non-complete.

#### Scenario: Partial refresh
- **WHEN** three leases are due and one provider request fails
- **THEN** two leases are refreshed, the failure is reported, and the command exits 2

### Requirement: Unheld blobs are reclaimed
Reclaiming SHALL delete every blob and derived content that no unexpired lease references and that is not protected by a lease's `retain` setting. Expired leases SHALL be removed before blobs are reclaimed. Releasing a lease and reacquiring to a new version SHALL trigger reclamation.

#### Scenario: Release frees content
- **WHEN** the only lease on a version is released
- **THEN** that version's blob is deleted, unless `retain` protects it

#### Scenario: Shared blob survives
- **WHEN** one of two leases referencing the same blob is released
- **THEN** the blob is kept

### Requirement: Concurrent bucket operations are serialized
Operations that change leases or blobs SHALL not interleave across processes sharing a bucket. A process SHALL wait for or fail on a concurrent writer rather than corrupt state, and an interrupted operation SHALL leave the previous consistent state.

#### Scenario: Two acquisitions at once
- **WHEN** two processes acquire different specs in the same bucket simultaneously
- **THEN** both leases are recorded and neither overwrites the other
