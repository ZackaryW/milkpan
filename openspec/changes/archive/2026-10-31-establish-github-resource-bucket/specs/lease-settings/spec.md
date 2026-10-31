# Spec Delta

## Purpose

Set how long a lease lasts, how often it refreshes and how many previous versions it keeps through `lease` obligations in the same policy that grants access, carried out by a built-in hook.

## ADDED Requirements

### Requirement: Lease settings come from the lease obligation
An allow decision MAY carry a `lease` obligation with any of the parameters `ttl`, `refresh` and `retain`. A built-in hook SHALL apply them to the lease being acquired, renewed or reacquired, and SHALL record them on the lease. A parameter that no obligation sets SHALL take its built-in default: `refresh` 1 hour, `ttl` 7 days, `retain` 0. Durations SHALL use the suffixes `s`, `m`, `h` or `d`.

#### Scenario: No lease obligation
- **WHEN** an allow decision carries no `lease` obligation
- **THEN** the lease expires 7 days later and the next acquisition after 1 hour re-resolves

#### Scenario: Per-resource settings
- **WHEN** policy attaches `lease(ttl=1d)` to `github:octo/fast-*` and nothing to other resources
- **THEN** leases on `github:octo/fast-cli` expire after 1 day and other leases after 7 days

### Requirement: Conflicting lease settings fail the operation
When several `lease` obligations apply, parameters set by only one of them SHALL merge. A parameter set to different values by two obligations SHALL fail the operation with `milkpan.conflicting_obligation`, naming the parameter and both values, and leases SHALL stay unchanged.

#### Scenario: Merge
- **WHEN** one matching rule sets `lease(ttl=1d)` and another sets `lease(refresh=10m)`
- **THEN** the lease uses ttl 1 day and refresh 10 minutes

#### Scenario: Conflict
- **WHEN** two matching rules set `ttl` to `1d` and `7d`
- **THEN** acquisition fails with `milkpan.conflicting_obligation` naming `ttl`, `1d` and `7d`

#### Scenario: Invalid value
- **WHEN** a `lease` obligation sets `ttl` to `soon`
- **THEN** acquisition fails with `milkpan.unmet_obligation` naming the parameter

### Requirement: Retained versions protect previous content
With `retain` set to N on a lease, reclamation SHALL keep the N most recent versions previously leased for that scope and spec, in addition to the current one.

#### Scenario: Rollback copy kept
- **WHEN** a lease with `retain=1` is reacquired to a new commit
- **THEN** the previous commit's content is kept and an older one is reclaimed

### Requirement: Configuration works without a file
Milkpan SHALL run without a configuration file, using a built-in policy that allows every lease operation in scope `default` only, with no `lease` obligation. A configuration file SHALL define the policy as rules with optional obligations; any configured rule SHALL replace the built-in policy. An invalid file SHALL fail every command with `invalid_request` naming the file.

#### Scenario: First run
- **WHEN** milkpan runs with no configuration file
- **THEN** acquisitions in scope `default` succeed with default settings and acquisitions in any other scope are denied with evidence

#### Scenario: Broken configuration
- **WHEN** the configuration file has a syntax error
- **THEN** commands fail with `invalid_request` and details naming the file
