# Fingerprint fixtures

- **Owner:** Fingerprint engine maintainers.
- **Purpose:** Cover fingerprint-spec validation and deterministic spec-to-argv behavior.
- **Commit:** Authored specs and expected argument arrays only; no browser profile, host fingerprint, or runtime data.
- **Generate/fetch:** Create cases from `docs/contracts/fingerprint-spec.schema.json` and compare expected arguments with the installed kernel manifest; no external assets are fetched.

Each golden file refers directly to the fingerprint contract schema and uses synthetic values.
