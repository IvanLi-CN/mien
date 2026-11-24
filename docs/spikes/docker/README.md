# Mien Docker headful display and sandbox spike

**Status: fail / incomplete; the M0-2.3 gate is not met.** Runtime evidence is `evidence/run.log`; it was captured on an x86_64 Agent VM using the pinned Debian bookworm-slim base and verified binary archives. An initial image build missed `xz-utils`; after adding it, a container was created without AppArmor because the earlier runner allowed that path, exposing that profile loading is blocked in this nested Agent VM. The latest runner requires operator-installed `/etc/apparmor.d/mien-chromium` and refuses container creation if loading fails; its final run recorded that refusal.

## Evidence and outcomes

| Requirement | Outcome | Evidence / configuration / residual risk |
|---|---|---|
| linux/amd64 and fixed assets | pass | `uname -m=x86_64`, Docker server `amd64 29.10.0`. Chromium archive SHA-256 `70d239830332e5820aa34dfcb284161cac0429eee25da642830afe04bda717f4`; Xray ZIP SHA-256 `b651f7e1d9966ee2c1b1ee1bf8b52c72771824d3065dce96bbc8bcc19ed95f82`; both matched manifests before use. Debian base digest `sha256:e67eeead736ff7bbe258ef088bff683635c561b1fcfefad0806c1df4ab49ce0e`. |
| Xvfb `:99`, openbox, headful window | fail | The test process could not connect to the X server: `Authorization required, but no authorization protocol specified`; no Chromium window or geometry was measured. Entry point was revised to create an Xauthority cookie and log display readiness, but that correction has not been exercised. Xvfb screen dimensions are shared across profiles; the kernel manifest explicitly says window size is not screen resolution. |
| `/dev/shm` | fail for default, 1 GiB required as spike baseline | Docker default was `67108864` bytes (64 MiB); Chromium sandbox page launch failed. With explicit 1 GiB, the mount was present and reported `1073741824` bytes. Headful workload could not connect to X, so 1 GiB sufficiency under concurrency is not established. |
| Namespace sandbox | blocked | Headless Chromium `--dump-dom chrome://sandbox` exited successfully but returned the New Tab HTML, not sandbox status. An attempted PID check found that PID had already exited; no `--no-sandbox` flag was passed, but enforced state is unconfirmed. Mandatory refusal condition: don't start a profile unless a dedicated probe reports namespace sandbox `enforced`; never silently pass `--no-sandbox`. |
| seccomp and AppArmor | seccomp pass; AppArmor blocked | Docker inspect showed `ReadonlyRootfs=true`, user `10001:10001`, explicit 1 GiB shm, and the host-supplied seccomp JSON in `SecurityOpt`. AppArmor load failed: `Cache read/write disabled: interface file missing` and `unable to find a suitable fs in /proc/mounts`. That nested Agent VM host cannot load the profile, and the executed container did not use AppArmor. The runner now requires `/etc/apparmor.d/mien-chromium` to load successfully; no `unconfined` fallback. The image copy `/opt/mien/seccomp/chromium.json` is audit-only. |
| Hysteria2 UDP egress | unsupported | No Hysteria2 endpoint or test credentials supplied. UDP end-to-end was not asserted. |
| noVNC one-time token | blocked | Viewer is disabled by default and no host port is published. If `MIEN_VIEWER_TOKEN` is set, entrypoint refuses startup because token consumption and second-use rejection are not implemented. |
| Cold start / memory / concurrency | partial | Container creation cold-start: **351 ms**. Idle container: **147.6 MiB / 4 GiB (3.60%)**, 5 PIDs in `docker stats`. Browser process launch did not produce a headful window; two Chrome process rows were observed later but no concurrent per-instance usage is claimed. |
| `/data` retention / orphan processes | pass for idle container only | `/data/restart-marker` survived restart. Post-restart `ps` showed tini, Xvfb, and openbox, with no Chrome process. No claim for orphan cleanup after headful browsers. |

## Intended container configuration

- `debian:bookworm-slim` pinned to the digest above; UID/GID 10001; read-only root; `/data` named volume; `/tmp:rw,noexec,nosuid,size=1g`; `/dev/shm` 1 GiB; `tini`; cap-drop ALL; no-new-privileges; PID limit 512; memory 4 GiB; 2 CPUs; nofile 4096.
- Chromium seccomp profile is required at container creation from a host path. `/opt/mien/seccomp/chromium.json` is a read-only audit copy only.
- `mien-chromium` AppArmor profile must be loaded by the host operator before running the script. The nested VM did not support AppArmor profile loading. `apparmor=unconfined` is not used.
- Chromium is not launched with `--no-sandbox`. No successful runtime sandbox state is recorded; deployments must fail closed if their independent `chrome://sandbox` or equivalent probe cannot show enforced namespace sandbox.
- noVNC/x11vnc are off in the image entrypoint; no ports are published. Token-enabled startup is rejected until replay-safe authentication exists.

## Reproduction

Run on a Linux/amd64 Docker host with `curl`, `sha256sum`, `tar`, `unzip`, `apparmor_parser`, a working AppArmor policy filesystem, and the required host profile at `/etc/apparmor.d/mien-chromium`:

```sh
sh docs/spikes/docker/run.sh
```

`run.sh` verifies both asset digests against the manifests before the image build, uses only its exact named container/image/volume, retains `/data` across reruns, and writes sanitized build/runtime evidence to `evidence/run.log`. It refuses to create a container unless the host AppArmor profile loads. The successful security conclusions above apply only to the specific observed runtime settings; acceptance still requires rerunning with the required profile active and headful probes passing.

## Remaining blockers

- AppArmor profile load blocked by nested Agent VM host. A Docker host permitting the required profile is needed to test the actual combination.
- The Xauthority correction is unverified; headful window size and display-bound `screen.*` remain unmeasured.
- `chrome://sandbox` returned New Tab HTML rather than a readable enforcement report. Need an explicit Chromium sandbox status probe; until confirmed, launch must be refused.
- Hysteria2 requires a test endpoint and credentials; no end-to-end UDP result.
- noVNC one-use token consumption and replay rejection are not implemented or tested.
- Concurrent headful resource use, data retention and orphan-process behavior remain unproven for actual browser processes.
