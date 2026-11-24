# Proxy fixtures

- **Owner:** Proxy gateway maintainers.
- **Purpose:** Record safe protocol configuration and failure-case expectations for local, controlled test endpoints.
- **Commit:** Authored non-secret configuration examples only. Never commit credentials, live endpoints, private keys, share links, or production captures.
- **Generate/fetch:** Author examples from `docs/contracts/proxy-engine-manifest.schema.json` and the Xray mapping contract; run controlled local proxy services separately when integration testing.

No live proxy or external endpoint is required by the committed baseline.
