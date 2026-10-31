# Spec Delta

## ADDED Requirements

### Requirement: Reclamation skips content in use
When deleting unheld content fails because the platform reports it in use, reclamation SHALL keep it, report it, continue with other content, and try again on the next reclamation.

#### Scenario: Old binary still running
- **WHEN** reclamation tries to delete an old version whose program is still running on Windows
- **THEN** that version is kept and listed as in use, and other unheld content is deleted
