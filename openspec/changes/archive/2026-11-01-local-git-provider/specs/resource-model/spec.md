# Spec Delta

## ADDED Requirements

### Requirement: Stored versions are shared across selectors
A version fetched for one selector of a source SHALL be reused, without fetching again, for any other selector of the same source that resolves to the same version.

#### Scenario: Ref then commit
- **WHEN** `github:octo/tools@main` resolved to commit `abc…` was acquired, and `github:octo/tools@abc…` is acquired in another scope
- **THEN** no content is downloaded and both leases refer to the same blob
