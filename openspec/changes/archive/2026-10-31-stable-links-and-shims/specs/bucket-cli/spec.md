# Spec Delta

## ADDED Requirements

### Requirement: Link commands
The binary SHALL provide `link <lease> --name <name> [--file <path>]`, `unlink <name>` and `link list`, and SHALL register panned's generic `slot list`, `slot show` and `slot switch` over milkpan's links.

#### Scenario: Self-management
- **WHEN** a user acquires milkpan's own release asset, runs `milkpan link <lease> --name milkpan --file <binary>` and later `milkpan reacquire --due`
- **THEN** `<home>/bin/milkpan` runs the new release on its next start

#### Scenario: Slot commands
- **WHEN** a user runs `milkpan slot list` with one link
- **THEN** the result lists that link with its current and switchable versions
