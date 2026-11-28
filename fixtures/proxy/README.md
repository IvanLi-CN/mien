# Proxy fixtures

- **Owner:** Proxy gateway maintainers.
- **Purpose:** Record safe protocol configuration and failure-case expectations for local, controlled test endpoints.
- **Commit:** Authored non-secret configuration examples only. Never commit real credentials, live endpoints, private keys, real share links, or production captures.
- **Generate/fetch:** Author examples from `docs/contracts/proxy-engine-manifest.schema.json` and the Xray mapping contract; run controlled local proxy services separately when integration testing.

No live proxy or external endpoint is required by the committed baseline.

## `share-links.json`

Share-link import scenarios for `parse_share_link` (`REQ-PROXY-015`, `PROXY-010`).

- Every host uses a reserved example name or a documentation address range
  (`example.test`, `198.51.100.0/24`, `203.0.113.0/24`); every key, UUID and password is a
  synthetic placeholder. Nothing here connects anywhere.
- `cases[]` is a table: `name`, `link`, and `expect`. `expect` is either the full
  `ProxyDraft` projection (`kind`, `server`, `port`, `options`, `credential_fields`,
  `warnings`) or an error (`error` code plus the field name the message must contain).
- The table covers accepted and rejected shapes for `vless`, `vmess`, `ss`, `trojan` and
  `hysteria2`, plus unknown schemes, rejected `tuic`/`wireguard` links, bad base64/padding
  variants, unknown `security`, missing fields and oversized input. The oversized, non-UTF-8
  and boundary-length inputs are built in `crates/mien-core/tests/share_link.rs` because
  they cannot be stored as readable JSON here.
- Error expectations assert that the message names the offending field and never carries a
  credential value or the link itself.
## `e2e-scenarios.json`

Scenario table for `crates/mien-core/tests/proxy_e2e.rs`, which drives real HTTP traffic
through the shared engine into a controlled Xray server inbound (`REQ-PROXY-001`,
`REQ-PROXY-003`, `REQ-PROXY-005`, `REQ-PROXY-021`) and checks the rejected paths
(`REQ-PROXY-009`, `REQ-PROXY-014`).

- `scenarios[]` covers the four Tier A TCP protocols (`vless`, `vmess`, `shadowsocks` with
  `aes-256-gcm`, `trojan`). Each carries a synthetic `credentials` map and a distinct
  `rejectedCredentials` map so the negative control can reuse the same node.
- `originHostTemplate` is expanded per run into a name that no resolver outside the
  controlled server can resolve; the server maps it to `127.0.0.1` in its own `dns.hosts`.
  That resolvability gap is what makes a direct connection unable to explain the traffic.
- `expect` holds the accepted HTTP status, the ECHO body fragments, the required zero origin
  hits for rejected credentials, and the `>N` comparison on the server's own inbound uplink
  counter. Numbers are compared as growth, so they are stable across runs.
- Nothing here contains a usable credential or a reachable real endpoint.
