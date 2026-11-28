# Runtime fixtures

- **Owner:** Runtime maintainers.
- **Purpose:** Describe headful, headless, pseudo-headful, sandbox, and isolation-state checks.
- **Commit:** Authored state metadata and expected redacted observations only; never commit browser user data, profiles, screenshots, or runtime secrets.
- **Generate/fetch:** Produce cases from the runtime and UI contracts in a controlled local harness; clean temporary data after each run.

No browser binary or user-data directory belongs in this fixture tree.
