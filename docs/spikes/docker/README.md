# Mien Docker headful display and sandbox spike

**Status: display and namespace sandbox verified on a real linux/amd64 host; the M0-2.3 gate is partially met.** Runtime evidence is `evidence/run.log`; it was captured on an x86_64 Agent VM host (109) using the pinned Debian bookworm-slim digest and manifest-verified Chromium/Xray archives. This host cannot load a custom AppArmor profile, so the run used the fallback that `SECURITY.md` explicitly allows: `--security-opt apparmor=unconfined` with the accepted risk recorded in the log (`accepted_risk=1`). AppArmor is defense-in-depth, not the mechanism that enables Chromium's sandbox; the namespace sandbox is evidenced by process namespaces below. The runner still refuses to downgrade silently: without a loaded profile it only proceeds when `MIEN_ALLOW_UNCONFINED=1` is set.

## Evidence and outcomes

| Requirement | Outcome | Evidence / configuration / residual risk |
|---|---|---|
| linux/amd64 and fixed assets | pass | `uname -m=x86_64`, Docker server `amd64 29.10.0`. Chromium archive SHA-256 `70d239830332e5820aa34dfcb284161cac0429eee25da642830afe04bda717f4`; Xray ZIP SHA-256 `b651f7e1d9966ee2c1b1ee1bf8b52c72771824d3065dce96bbc8bcc19ed95f82`; both matched manifests before use. Debian base digest `sha256:e67eeead736ff7bbe258ef088bff683635c561b1fcfefad0806c1df4ab49ce0e`. |
| Xvfb `:99`, openbox, headful window | pass | `xdpyinfo` reports `1920x1080 pixels`, depth 24; `xwininfo -root -tree` shows `"about:blank - Chromium" ... 1280x720+0+0`, matching `--window-size=1280,720`. Probes must pass `XAUTHORITY=/tmp/mien-Xauthority`; without it the X server correctly refuses. Screen size comes from Xvfb and is host-bound; window size is not screen resolution. |
| `/dev/shm` | pass for the tested workload; 64 MiB default fails | With the Docker default (`67108864` bytes) Chromium fails to start (`default_64m_chromium=failed`). With the explicit 1 GiB mount (`1073741824` bytes) three headful instances run together. |
| Namespace sandbox | pass (process-level evidence) | `chrome://sandbox` via `--dump-dom` returns the New Tab HTML, so it is not a usable probe. `/proc` evidence is: init `user:[4026537903] pid:[4026538485]`; sandboxed zygotes `user:[4026538636] pid:[4026538565]`; renderer `user:[4026538636] pid:[4026538562]` with `Seccomp=2` and `NoNewPrivs=1`; no `--no-sandbox` in any cmdline. One zygote intentionally runs as `--no-zygote-sandbox`, which is Chromium's own layout and does not mean the renderer is unsandboxed. Refusal rule stays: never silently pass `--no-sandbox`. |
| seccomp and AppArmor | seccomp pass; AppArmor via documented fallback | Docker inspect: `ReadonlyRootfs=true`, user `10001:10001`, shm `1073741824`, host-supplied seccomp JSON in `SecurityOpt`, `no-new-privileges`. Profile load is unavailable on this host, so the run used `apparmor=unconfined` with `accepted_risk=1` recorded. AppArmor is not required for Chromium's namespace sandbox; it is extra LSM confinement and is required only because Docker's `docker-default` profile denies `mount`/`pivot_root`. The image copy `/opt/mien/seccomp/chromium.json` is audit-only. |
| Hysteria2 UDP egress | unsupported | No Hysteria2 endpoint or test credentials supplied. UDP end-to-end was not asserted. |
| noVNC one-time token | blocked | Viewer is disabled by default and no host port is published. If `MIEN_VIEWER_TOKEN` is set, entrypoint refuses startup because token consumption and second-use rejection are not implemented. |
| Cold start / memory / concurrency | pass (3 instances) | Container creation cold-start: **4229 ms** (first run after image build; a warm rerun measured 3456 ms). Three headful instances: `cpu=0.01%`, `mem=190.4 MiB / 4 GiB (4.65%)`, `pids=96`, `chrome_process_count=13`. |
| `/data` retention / orphan processes | pass | `/data/restart-marker` survived restart (`data_persistence=pass`). After three headful instances the restart showed tini, Xvfb and openbox with no Chrome process (`restart_orphans=none_observed`). |

## Intended container configuration

- `debian:bookworm-slim` pinned to the digest above; UID/GID 10001; read-only root; `/data` named volume; `/tmp:rw,noexec,nosuid,size=1g`; `/dev/shm` 1 GiB; `tini`; cap-drop ALL; no-new-privileges; PID limit 512; memory 4 GiB; 2 CPUs; nofile 4096.
- Chromium seccomp profile is required at container creation from a host path. `/opt/mien/seccomp/chromium.json` is a read-only audit copy only.
- AppArmor has two sanctioned host configurations (`SECURITY.md` §宿主侧沙箱): load the `mien-chromium` profile, or run `--security-opt apparmor=unconfined` and record it as accepted risk. The nested VM could not load the profile, so the fallback must be selected explicitly (`MIEN_ALLOW_UNCONFINED=1`); it is never the default and never silent.
- Chromium is not launched with `--no-sandbox`. Namespace sandbox state is verified from `/proc` (distinct user/pid namespaces plus `Seccomp=2`); deployments must still fail closed if their own probe cannot confirm enforcement. probe cannot show enforced namespace sandbox.
- noVNC/x11vnc are off in the image entrypoint; no ports are published. Token-enabled startup is rejected until replay-safe authentication exists.

## Reproduction

Run on a Linux/amd64 Docker host with `curl`, `sha256sum`, `tar`, `unzip`, and either a working AppArmor policy filesystem plus `/etc/apparmor.d/mien-chromium`, or `MIEN_ALLOW_UNCONFINED=1` to record the documented fallback:

```sh
sh docs/spikes/docker/run.sh
```

`run.sh` verifies both asset digests against the manifests before the image build, uses only its exact named container/image/volume, retains `/data` across reruns, and writes sanitized build/runtime evidence to `evidence/run.log`. It refuses to create a container unless the host AppArmor profile loads. The successful security conclusions above apply only to the specific observed runtime settings; acceptance still requires rerunning with the required profile active and headful probes passing.

## Remaining blockers

- The `mien-chromium` AppArmor profile itself has not been loaded on any host yet; this run used the documented `unconfined` fallback with recorded risk. A host that supports profile loading is still required for the hardened path.
- `chrome://sandbox` is not a usable probe in this build (returns the New Tab page). The process-level `/proc` probe is the evidence of record; a product probe still has to be implemented before startup can be gated on it.
- Hysteria2 requires a test endpoint and credentials; no end-to-end UDP result.
- noVNC one-use token consumption and replay rejection are not implemented or tested.
- Only three concurrent headful instances were exercised; the 10-profile capacity target and 24-hour soak remain untested.
