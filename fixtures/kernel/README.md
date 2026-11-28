# Kernel fixtures

- **Owner:** Mien runtime maintainers.
- **Purpose:** Validate Chromium 148 capability manifests and runtime admission without distributing kernel binaries.
- **Commit:** Authored JSON manifests, candidate metadata, and expected validation results only. Never commit executables, release archives, or caches.
- **Generate/fetch:** Copy and review a manifest from `docs/contracts/manifests/`; binary acquisition stays external and is handled by the locked release asset workflow, not this fixture set.

The directory is intentionally a metadata-only placeholder until the kernel manifest contract gains a stable fixture case. The root provenance lock records a schema-backed candidate manifest and its pinned upstream asset digest; it does not imply that the kernel archive was downloaded.
