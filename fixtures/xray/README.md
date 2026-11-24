# Xray fixtures

- **Owner:** Proxy gateway maintainers.
- **Purpose:** Validate deterministic Xray inbound, outbound, route, lifecycle, and API-boundary mappings.
- **Commit:** Authored redacted mapping inputs and expected JSON only; never commit Xray binaries, container images, credentials, or private keys.
- **Generate/fetch:** Generate mappings from `docs/XRAY-MAPPING.md` and the pinned engine manifest; obtain the engine externally through the release workflow.

The engine API is expected to remain loopback-only; committed fixtures must not contain a usable management endpoint.
