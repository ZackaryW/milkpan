# Spec Delta

## MODIFIED Requirements

### Requirement: Bucket location is configurable
The bucket's state, blobs and configuration SHALL live under one home directory, chosen by `--home`, then `MILKPAN_HOME`, then `.milkpan` in the user's home directory. A bucket in another location SHALL NOT be read or migrated implicitly.

#### Scenario: Isolated bucket
- **WHEN** a command runs with `MILKPAN_HOME` set to an empty directory
- **THEN** it reads and writes only inside that directory

#### Scenario: Default home
- **WHEN** a command runs with neither `--home` nor `MILKPAN_HOME`
- **THEN** it uses `~/.milkpan`, creating it on first write
