# Profile fixtures

- **Owner:** Profile management maintainers.
- **Purpose:** Exercise export/import contract compatibility without customer profiles.
- **Commit:** Authored, redacted profile payloads only; no credentials, cookies, browser data, or customer exports.
- **Generate/fetch:** Build fixtures from `docs/contracts/profile-export.schema.json` with a fresh synthetic profile ID and schema-valid fingerprint; do not fetch production data.

This directory currently documents the fixture boundary; profile export payload coverage will be added alongside import/export implementation.
