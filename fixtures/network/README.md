# Network fixtures

- **Owner:** Network security maintainers.
- **Purpose:** Cover IPv6, WebRTC, DoH, and egress-path checks using controlled local targets.
- **Commit:** Authored scenario descriptions and expected redacted observations only; no public-site snapshots, credentials, or host telemetry.
- **Generate/fetch:** Start an isolated local echo/canary service from the test harness; regenerate expected observations from the network contract.

The baseline intentionally has no network service or packet capture checked in.
