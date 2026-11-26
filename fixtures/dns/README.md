# DNS fixtures

- **Owner:** Network security maintainers.
- **Purpose:** Define local DNS canary and blocked-resolution scenarios for remote-DNS and failure-classification tests.
- **Commit:** Authored scenario metadata and synthetic observations only; never commit real resolver logs or user network data.
- **Generate/fetch:** Use local test resolver/container definitions from the test harness and synthetic names under `.test`; do not query production DNS services.

## Contents

- `canary-scenarios.json` — the canary resolver contract (UDP+TCP on a loopback port, `*.canary.test` answers with fixed A/AAAA, every other name `NXDOMAIN`) plus authored expected-name / observed-query pairs covering all three verdicts, including the case-and-trailing-dot normalisation.

`crates/mien-core/tests/network_canary.rs` serves exactly this contract from a `UdpSocket` and a `TcpListener` on the same port, and the observations stay in-process.

A resolver that sees no query is *not* evidence that none happened: on macOS the platform resolves out of process, so the canary alone stays `inconclusive` there. See `docs/spikes/network/README.md`.
