# DNS fixtures

- **Owner:** Network security maintainers.
- **Purpose:** Define local DNS canary and blocked-resolution scenarios for remote-DNS and failure-classification tests.
- **Commit:** Authored scenario metadata and synthetic observations only; never commit real resolver logs or user network data.
- **Generate/fetch:** Use local test resolver/container definitions from the test harness and synthetic names under `.test`; do not query production DNS services.

The committed baseline contains only fixture documentation and provenance metadata; no resolver binary or captured traffic is stored.
