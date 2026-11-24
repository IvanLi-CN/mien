# Database fixtures

- **Owner:** Storage maintainers.
- **Purpose:** Verify empty-database migration behavior and schema initialization.
- **Commit:** SQL migration expectations and synthetic schema metadata only; never commit customer databases, WAL files, or secrets.
- **Generate/fetch:** Run the repository migrations against a temporary SQLite database and compare the expected version and invariant tables; clean the temporary database after the run.

The baseline checks that an empty database reaches migration version 1 and records the initial schema migration.
