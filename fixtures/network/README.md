# Network fixtures

- **Owner:** Network security maintainers.
- **Purpose:** Cover IPv6, WebRTC, DoH, and egress-path checks using controlled local targets.
- **Commit:** Authored scenario descriptions and expected redacted observations only; no public-site snapshots, credentials, or host telemetry.
- **Generate/fetch:** Start an isolated local echo/canary service from the test harness; regenerate expected observations from the network contract.

## Contents

- `canary-scenarios.json` — authored WebRTC candidate sets, IPv6 route observations and resolver-state observations, each with the verdict it must produce. Together they pin all three verdict states per control, so neither "not observed" nor "observed absent" can be silently collapsed into the other.

The rules themselves live in `crates/mien-core/src/network/mod.rs`; the fixtures are its regression corpus. Measured kernel behaviour and its limits are recorded in `docs/spikes/network/README.md`, not here.
