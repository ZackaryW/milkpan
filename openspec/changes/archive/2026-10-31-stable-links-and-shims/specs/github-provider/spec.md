# Spec Delta

## ADDED Requirements

### Requirement: Archive release assets are extracted
A release asset whose name ends in `.tar.gz`, `.tgz` or `.zip` SHALL be stored as an archive and presented as a directory tree, with a single top-level directory stripped when every entry shares it. Other assets SHALL remain single files.

#### Scenario: Zipped binary
- **WHEN** the asset `tool-windows.zip` containing `tool-1.0/tool.exe` is acquired
- **THEN** the lease's path is a directory containing `tool.exe`

#### Scenario: Plain binary asset
- **WHEN** the asset `tool-linux` is acquired
- **THEN** the lease's path is that file
