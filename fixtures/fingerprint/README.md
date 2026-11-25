# Fingerprint fixtures

- **Owner:** Fingerprint engine maintainers.
- **Purpose:** Cover fingerprint-spec validation, deterministic argv construction, and explicitly rejected capabilities.
- **Commit:** Authored synthetic specs and expected arguments only; no browser profile, host fingerprint, credentials, or runtime data.
- **Generate/fetch:** Golden cases are checked against the canonical installed-kernel manifest and repeated deterministically; no external assets are fetched.

Golden `expected_argv` arrays are exact process argument arrays. Runtime network controls are asserted separately with an enforcement status; they are never inferred from Chromium flags.
