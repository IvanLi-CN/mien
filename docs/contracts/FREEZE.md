# Mien contract freeze

- Freeze version: `M0-contract-freeze`
- Freeze source base: `0bfc28052d1eb1af7b29c8fb8798f21155f3209d`.
- Generated at: `2026-11-24T23:06:10Z` (UTC).

## Frozen files and digests

The `contracts:check` job computes SHA-256 digests for the frozen source files and compares generated client output against its checked-in snapshot. The current freeze digest receipt is below; a changed contract must update this receipt in the same change.

| File | SHA-256 |
| --- | --- |
| `docs/contracts/openapi.yaml` | `42a76a18bbcd286c2ae9b62883f8ceb716afbce62ae8ca5a87aa911d65599dd2` |
| `docs/contracts/events.schema.json` | `bb6a7cb24d11f6ac8d0cf3922f2405a25e7e71207b9b8649a78312e1daff207b` |
| `docs/contracts/fingerprint-input.schema.json` | `58a658c61a6c222e51f13af81b02939e8f13065991e54cc391e5abf9d0c0fcc7` |
| `docs/contracts/fingerprint-spec.schema.json` | `836ab6c56e8aee8737c64511ed80ccac68b6f08507dae98c413167915f407725` |
| `docs/contracts/profile-export.schema.json` | `4a532d7f903b6ac05c63508169a54ec76805fcf9a5b34fe614720d05615a6f96` |
| `docs/contracts/kernel-manifest.schema.json` | `9bec784b845b5395395bc86ad2621f0af38ff16b042705251321dd134869bf9d` |
| `docs/contracts/proxy-engine-manifest.schema.json` | `3908f9c1f9afd10e8480acfc861833ac08c0cd18c7cf99ee4817865ab4496a00` |
| `docs/ERROR-CODES.md` | `9ed8793afdd286a3f0bafdcc025968e9fdc33d6da129a7e0c87d5dbcad23c3fd` |
| `docs/DATABASE-SCHEMA.md` | `07393833278cfa33d9506f11c92a4b39c10daa7344da3a0a4a722e786cfe8d6c` |
| `migrations/0001_init.sql` | `793bb208f537caa1184c97f5400040245d6b16da0d3ab7b937ee788c6ffcd0d2` |
| `docs/XRAY-MAPPING.md` | `551da3edb61683c903d8e63cf9cdd88885e1fbaca1dbc001306401d427803608` |
| `docs/UI-SURFACES.md` | `fadcd599db8d6429d2942d584af144c100d5417d4101d439ac4dd83c9d88917e` |
| `docs/TEST-FIXTURES.md` | `29d2335339ffa860e3afad0039b4db8576328d1d0507293d93082776d9aaa67e` |
| `deploy/docker/seccomp/chromium.json` | `43cf666b8a0eb6490dc623e8ea49758146fc5cd6233ac38b16b181146fdfcfd` |

Digest table above covers individual files; `docs/contracts/manifests/` and `deploy/docker/seccomp/` directories are additionally checked by `contracts:check` for schema validity, structure and provenance.

## Evidence applied

- T3 Xray confirmed the HandlerService/RoutingService CRUD primitives, loopback binding, block-first failure-closed bootstrap, shared VLESS outbound, and bounded kill/rebuild behavior. Xray native CRUD is not idempotent: gateway reconciliation must map duplicate add and unknown remove to idempotent success; mux concurrency is normalized by v26.11.9 to `128`; Hysteria2 requires `protocol: hysteria`, `version: 2`, `streamSettings.network: hysteria`, and `hysteriaSettings`.
- T3 accepted the Hysteria2 configuration schema only; no controlled UDP/TLS peer was available, so Hysteria2 is not frozen as end-to-end connectivity. TUIC is rejected as `unknown config id: tuic` and remains outside v1.
- T4 verified fingerprint/platform/language/timezone/window/CPU, Edge and Opera brand samples, and audio/canvas/GPU spoof effects on the pinned macOS arm64 build. `clientrects`, `font`, proxy-server, non-proxied UDP, Chrome/Vivaldi brand behavior, independent platform-version behavior, and sandbox/Seatbelt effectiveness remain unverified; pseudo-headful changes UA only.
- T5 Docker conclusions are a known block: nested Agent VM could not load the AppArmor profile, headed X connection failed, and `chrome://sandbox` did not provide a determinable enforced result. Container sandbox must not be described as enforced before evidence exists; noVNC one-time tokens are not implemented; Hysteria2 UDP is not end-to-end verified. These do not block the macOS headed M1 path.

## Change process after freeze

Any contract change must update the owning SPEC, implementation VER record, executable test or fixture, and affected user-facing documentation in one change. The `contracts` job must pass on the same commit; generated client output, error-code mappings, UI route parity, DDL/migration checks, and seccomp provenance are blocking checks. Do not edit a generated or derived file alone; update its source and regenerate the snapshot.

## Known blocks

- Docker sandbox enforcement is unconfirmed; M3 Docker release material must not claim an enforced sandbox.
- noVNC one-time viewer tokens are not implemented.
- Hysteria2 UDP connectivity was not tested end to end.
- `--disable-spoofing=clientrects|font` had no observed target effect in T4 and is not a verified capability; pseudo-headful changes UA only and retains other Headless characteristics.

## To adjudicate

- The locked PRD/architecture/test-plan material describes future Docker and protocol surfaces more broadly than the measured M0 evidence. This freeze records the measured boundaries without changing those locked documents; product scope and release timing must be adjudicated before promoting unverified capabilities.
