# Engine fixtures

- **Owner:** Proxy gateway maintainers.
- **Purpose:** Pin engine release-manifest metadata used by admission and compatibility checks.
- **Commit:** Manifest metadata and release digests only. Do not commit Xray executables, archives, container images, or credentials.
- **Generate/fetch:** Copy the schema-valid candidate manifest from `docs/contracts/manifests/`; `xtask fixtures fetch` is the only download path and uses a separate cache.

The baseline records the upstream release URL and asset digest without downloading the binary.
