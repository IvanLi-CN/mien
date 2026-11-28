# Gate coverage map

`release-gate.json` reserves 169 IDs (`docs/TEST-PLAN.md:320`). Every one is
currently `missing` because no report has been produced. This map records where
each family's evidence would have to come from, and — more usefully — which
families have no source at all.

It is **not** a gate report. Nothing here marks an ID as `pass`: a passing ID
needs a report bound to a `source_sha` with per-assertion results, which does
not exist yet. Treat this page as the route to filling that report, not as
evidence that it is filled.

## Gate composition

169 IDs = 159 defined in `docs/TEST-PLAN.md` + 10 `PRD-ACC` defined in
`docs/PRD.md`. Three IDs exist in the plan but are deliberately **not** gate
items: `PROXY-007` and `PROXY-030` (Tier B), `PERF-008` (non-blocking). The
checker rejects them if they appear (`scripts/release/gate.mjs`).

## Where each family's evidence lives today

Test counts are `#[test]` / `#[tokio::test]` attributes in the files a family
draws on. A file can serve several families, so the column sums past 205.

| IDs | Count | Domain | Candidate sources | Tests |
| --- | --- | --- | --- | --- |
| `FP-001..013` | 13 | fingerprint | `tests/fingerprint_consistency.rs`, `src/fingerprint/` | 22 |
| `KERNEL-001..010` | 10 | kernel capabilities | `tests/mode_matrix.rs`, `docs/spikes/kernel/` | 4 |
| `ENGINE-001..009` | 9 | protocol engine | `tests/xray_integration.rs`, `tests/proxy_e2e.rs` | 13 |
| `PROXY-001..006`, `008..029` | 28 | proxy | `tests/proxy_storage.rs`, `tests/share_link.rs`, `tests/proxies_api.rs`, `tests/network_canary.rs` | 29 |
| `EXPORT-001..006` | 6 | import/export | `tests/export_import.rs` | 11 |
| `UI-001..022` | 22 | web UI | `src/`, `src-tauri/tests/acceptance.rs` | 1 |
| `RT-001..026` | 26 | runtime | `tests/runtime_integration.rs`, `tests/reconcile.rs`, `tests/admission.rs`, `tests/slice_e2e.rs`, `tests/shutdown_stop.rs` | 24 |
| `API-001..020` | 20 | control API | `tests/http.rs`, `tests/profiles_api.rs`, `tests/auth.rs`, `tests/tokens.rs`, `tests/events.rs`, `tests/audit_api.rs`, `tests/static_ui.rs` | 85 |
| `SEC-001..009` | 9 | security baseline | `tests/auth.rs`, `src/runtime/probe.rs`, `docs/SECURITY.md` | 16 |
| `PKG-001..009` | 9 | packaging | `scripts/release/`, `.github/workflows/` | 3 |
| `PERF-001..007` | 7 | performance | — | **0** |
| `PRD-ACC-001..010` | 10 | end-to-end acceptance | — | **0** |

## The three empty families

**23 of 169 IDs have no test source whatsoever.** They are not "probably
covered somewhere"; there is nothing to point at.

- `PKG-001..009`: release packaging. Three IDs now have a real assertion and six
  do not:
  - `PKG-003` (updater signature): `scripts/release/updater-signature.test.mjs`
    plus `macos-adhoc.sh --updater-public-key`. The verifier parses minisign
    keys and signatures, distinguishes `Ed` (raw) from `ED` (BLAKE2b-512
    prehash), and rejects unsigned, tampered, wrong-key and downgrade
    manifests. Verified end to end against a real `tauri signer` key and a real
    `.app.tar.gz`. **What it does not cover**: nothing has been signed by a
    production key, because no key is configured yet.
  - `PKG-008` (release notes): `scripts/release/release-notes.test.mjs` and
    `docs/KNOWN-LIMITATIONS.md`. The checker rejects a missing category or a
    category with no substance, and the repository documents are asserted to
    pass. **What it does not cover**: the text itself is documentation; nothing
    proves a shipped release contains it.
  - `PKG-009` (GitHub Actions release): `scripts/release/release-policy.mjs`,
    `rehearsal.mjs`, `release-manifest.mjs`, `github-facts.mjs` and their tests,
    all called by `release.yml`. The policy rejects a missing or invalid
    `release_target`, a target not greater than the trusted baseline, a version
    collision outside the exact-identity match, a reservation that disagrees on
    version or source, a source that is not merged-and-green on `CI Main`, an
    artifact manifest missing any required entry, an attestation that covers
    different bytes, and any Apple certificate or undeclared secret.
    **What it does not cover**: nothing builds or publishes. Every job is
    `contents: read`, `publish` is `exit 1`, and draft reservation creation
    needs a write token no job requests. The rehearsal proves the policy, not
    a release.
  - `PKG-001`, `PKG-002`, `PKG-004`, `PKG-005`, `PKG-006` and `PKG-007`
    remain without a test source. `PKG-001`/`PKG-002` need a real signed
    `.app` and DMG; `PKG-005` needs Docker, which is disabled on this host;
    `PKG-007` is currently a **failure**, not a gap: 12 distinct
    HIGH/CRITICAL CVEs in the pinned base image (see
    `deploy/docker/security-exceptions.yaml`), and the list is deliberately
    empty.
- `PERF-001..007`: no benchmark or load test exists in the repository.
  `PRD-ACC-009` also requires these to pass, so it cannot pass either.
- `PRD-ACC-001..010`: the end-to-end acceptance runs. Some *parts* are covered
  by narrower tests (headful launch, headless degradation, export/import), but
  `ACC` requires evidence per criterion, bound to a release candidate. Nothing
  binds them.

## Two families with thin coverage relative to their ID count

- `UI-001..022` — 22 IDs, and the only automated coverage is one acceptance
  test plus Vitest component tests under `src/`. Most UI criteria are manual
  (`docs/TEST-PLAN.md` marks several as such), which is allowed, but a manual
  criterion still needs a recorded result bound to a build.
- `KERNEL-001..010` — the spike recorded capability differences in
  `docs/spikes/kernel/`, but that is prose plus measurements, not a test that
  re-runs.

## What filling this requires

1. For each ID, decide whether its criterion is automated or manual.
2. Automated: produce a per-assertion result file and bind it to a `source_sha`.
3. Manual: record who ran it, on which build, with what outcome.
4. Write the entries and let `xtask release-gate --check` decide. It rejects a
   `pass` without evidence, so the report cannot be filled in optimistically.
