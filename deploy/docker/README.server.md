# Production server image (`Dockerfile.server`)

Runs `mien-server`, the static UI, and the shared Xvfb display stack as one
non-root container. The display and sandbox layout is the one the M0 spike
verified on a real `linux/amd64` host (`docs/spikes/docker/`); this image swaps
the spike's `sleep infinity` for the real server and keeps everything else.

## What is in the image

| Path                              | Content                                                           |
| --------------------------------- | ----------------------------------------------------------------- |
| `/usr/local/bin/mien-server`      | release server binary, built outside the image                    |
| `/opt/mien/web`                   | `dist/` static UI, served by `MIEN_WEB_DIST`                      |
| `/opt/mien/chromium/chrome`       | pinned fingerprint kernel, `MIEN_KERNEL_BIN`                      |
| `/opt/mien/xray/xray`             | pinned proxy engine, `MIEN_XRAY_BIN`                              |
| `/opt/mien/seccomp/chromium.json` | audit copy only; Docker loads the host-side file                  |
| `/data`                           | volume: `mien.db`, `secrets/master.key`, per-profile browser dirs |

The base image is `debian@sha256:e67eeead736ff7bbe258ef088bff683635c561b1fcfefad0806c1df4ab49ce0e`,
the same digest `Dockerfile.spike` and `docs/spikes/docker/evidence/run.log`
used. It is a digest, not a tag, so a rebuild cannot silently change libc under
the kernel.

## Build inputs

Neither the kernel nor Xray is downloaded inside the image: both are staged in as
already-verified archives and the build re-checks their SHA-256, so a wrong or
tampered file fails the build.

| Build arg          | Default             | Digest source                                                                               |
| ------------------ | ------------------- | ------------------------------------------------------------------------------------------- |
| `CHROMIUM_ARCHIVE` | `chromium.tar.xz`   | `docs/contracts/manifests/fingerprint-chromium-148.0.7778.215-linux-x86_64.json` → `sha256` |
| `CHROMIUM_SHA256`  | `70d23983…bda717f4` | same manifest; the digest the spike recorded as matched                                     |
| `XRAY_ARCHIVE`     | `xray.zip`          | `docs/contracts/manifests/xray-core-v26.11.9-linux-x86_64.json` → `sha256`                  |
| `XRAY_SHA256`      | `b651f7e1…ed95f82`  | same manifest                                                                               |
| `SERVER_BINARY`    | `mien-server`       | build artifact, see below                                                                   |

Both manifests are frozen in `docs/contracts/FREEZE.md`; their own digests are
checked by `contracts:check`. The release URLs the spike uses are
`https://github.com/adryfish/fingerprint-chromium/releases/download/148.0.7778.215/ungoogled-chromium-148.0.7778.215-1-x86_64_linux.tar.xz`
and
`https://github.com/XTLS/Xray-core/releases/download/v26.11.9/Xray-Linux-64.zip`
(`Xray-linux-64.zip`).

### Build the two artifacts

`mien-server` is cross-built with `cargo-zigbuild`, which needs no Linux
toolchain on the build host:

```sh
cargo zigbuild --release -p mien-server --target x86_64-unknown-linux-gnu
bun install --frozen-lockfile
bun run build
```

The binary is dynamically linked only against `libc`, `libm`, `libdl` and
`libpthread` (glibc ≤ 2.29), so Debian bookworm's glibc runs it unchanged.

### Build the image

Stage the artifacts beside the Dockerfile, then build:

```sh
mkdir -p /tmp/mien-server-image/context
cp deploy/docker/Dockerfile.server deploy/docker/entrypoint.sh \
   deploy/docker/healthcheck.sh /tmp/mien-server-image/context/
mkdir -p /tmp/mien-server-image/context/seccomp
cp deploy/docker/seccomp/chromium.json /tmp/mien-server-image/context/seccomp/
cp -r dist /tmp/mien-server-image/context/dist
cp target/x86_64-unknown-linux-gnu/release/mien-server /tmp/mien-server-image/context/mien-server
cp chromium.tar.xz xray.zip /tmp/mien-server-image/context/

docker build \
  -f /tmp/mien-server-image/context/Dockerfile.server \
  -t mien-server:local \
  /tmp/mien-server-image/context
```

## Run

```sh
docker run -d --name mien-server \
  --read-only --tmpfs /tmp:rw,noexec,nosuid,size=1g --shm-size=1g \
  --cap-drop ALL --security-opt no-new-privileges \
  --security-opt seccomp=$PWD/deploy/docker/seccomp/chromium.json \
  --security-opt apparmor=unconfined \
  --user 10001:10001 --pids-limit 512 --memory 4g --cpus 2 \
  --ulimit nofile=4096:4096 \
  -p 127.0.0.1:8787:8787 \
  -v mien-server-data:/data \
  -e MIEN_BIND=0.0.0.0:8787 \
  -e MIEN_PUBLIC_URL=http://127.0.0.1:8787 \
  mien-server:local
```

`MIEN_BIND` and `MIEN_PUBLIC_URL` are a pair. A non-loopback bind without
`MIEN_PUBLIC_URL` is rejected at startup by `validate_bind`
(`crates/mien-server/src/state.rs`), and `MIEN_PUBLIC_URL` also drives the
`Origin` check on Cookie writes and the `Secure` attribute on the session
cookie, so the value must be the exact origin the browser uses (`http://` for a
localhost port, `https://` behind TLS). Both are set as image defaults with the
loopback origin; override them for a real host.

`--shm-size=1g` and the writable `/tmp` tmpfs are runtime requirements: with the
Docker default 64 MiB `/dev/shm` Chromium fails to start (spike evidence), and
`XDG_RUNTIME_DIR`/`XAUTHORITY` live in `/tmp`.

### AppArmor

`--security-opt apparmor=unconfined` above is the documented fallback from
`deploy/docker/apparmor/README.md`, not a verified hardened path. The
`mien-chromium` profile still has not been loaded on any available host, because
`securityfs` is not mountable in the unprivileged test hosts. Replace the flag
with `--security-opt apparmor=mien-chromium` only on a host where the profile
actually loads. Do not record the AppArmor path as verified until then.

### Environment

| Variable                   | Default in image            | Meaning                                                                             |
| -------------------------- | --------------------------- | ----------------------------------------------------------------------------------- |
| `MIEN_BIND`                | `0.0.0.0:8787`              | listen address                                                                      |
| `MIEN_PUBLIC_URL`          | `http://127.0.0.1:8787`     | required for a non-loopback bind; also the Cookie `Origin`                          |
| `MIEN_DATA_ROOT`           | `/data`                     | volume root                                                                         |
| `MIEN_WEB_DIST`            | `/opt/mien/web`             | static UI root                                                                      |
| `MIEN_KERNEL_BIN`          | `/opt/mien/chromium/chrome` | kernel executable                                                                   |
| `MIEN_XRAY_BIN`            | `/opt/mien/xray/xray`       | shared engine executable                                                            |
| `MIEN_ALLOW_DIRECT_EGRESS` | unset                       | `1` allows direct-egress profiles; otherwise start returns `DIRECT_EGRESS_DISABLED` |
| `MIEN_ADMIN_PASSWORD_FILE` | unset                       | 0600 file with the bootstrap password                                               |
| `MIEN_MASTER_KEY`          | unset                       | hex master key; overrides `secrets/master.key`                                      |
| `MIEN_SCREEN_SIZE`         | `1920x1080x24`              | Xvfb geometry, shared by all profiles                                               |

## Data volume

`/data` holds `mien.db`, `secrets/master.key`, `secrets/admin-bootstrap`, and one
directory per profile under `profiles/`. `/data`, `/data/secrets` and
`/data/profiles` are `0700`; the master key is `0600` and owned by `10001:10001`
(`MasterKey::from_file` refuses anything wider).

On first start with an empty `/data` the entrypoint provisions
`secrets/master.key` (`0600`, 32 CSPRNG bytes) and the server generates
`secrets/admin-bootstrap`. It never silently replaces a key when `mien.db`
already exists: that would invalidate every stored proxy credential and API
token. Restore the file or set `MIEN_MASTER_KEY` instead.

A volume created by Docker is owned by the image's user; a bind mount from the
host is not. `chown -R 10001:10001 <host-dir>` before the first start, or the
entrypoint stops with `data_root_unusable=permissions`.

## Health check

`HEALTHCHECK` runs `healthcheck.sh`, whose exit codes are contractual:

| Exit | Meaning                                                                             |
| ---- | ----------------------------------------------------------------------------------- |
| `0`  | `/healthz` 200 and `/readyz` 200                                                    |
| `1`  | `/healthz` 200 and `/readyz` 503 — a documented not-ready answer, not a probe fault |
| `2`  | `/healthz` not 200, or `/readyz` answered anything else, or no response             |

`--start-period=90s` covers first-start migration to `0008` plus Xvfb and
openbox startup.

## Acceptance

```sh
docker exec mien-server /usr/local/bin/verify-server.sh
```

The script must be copied into the image before that command works; it runs as
the container user and prints one `PASS`/`FAIL` line per assertion: health,
login, forced password change, profile creation with a complete fingerprint,
headful start to `running` with the reported `isolation`, the kernel process and
its seccomp/user-namespace evidence from `/proc`, stop, relaunch, final stop, no
root processes, data permissions, the migration version, a `headless` profile
whose real `/proc/<pid>/cmdline` carries `--headless=new` and no headful window
flags, and a `running` probe left behind for the restart phases.

Restart reconciliation cannot be asserted from inside the container that is
restarted, so `verify-restart.sh` drives it from the Docker host. It starts the
container with the run shape above, runs the in-container acceptance, then:

1. `docker restart`, re-checks `/healthz` and `/readyz` `200`, that the profile
   survives, that the runtime row is terminal, that `last_known_isolation` is
   retained, and that no kernel process remains;
2. `docker kill` (SIGKILL, so the entrypoint's cleanup never runs) and
   `docker start`, then asserts the new process logged
   `runtime reconciliation finished`, the leftover `running` row settled to a
   terminal state, orphan kernel processes are `0`, and both probes answer
   `200`.

```sh
deploy/docker/verify-restart.sh mien-server:local
```

The driver removes its own container and volume on exit. It requires `curl` and
`docker` on the host, and reads `MIEN_VERIFY_PASSWORD` / `MIEN_VERIFY_NEW_PASSWORD`
only to keep a reused volume usable; a clean volume needs neither.

`verify-server.sh` takes a phase from `MIEN_VERIFY_PHASE`: `full` (default)
runs the acceptance, `check` asserts only the post-restart state and reads the
profile id from `/data/verify-restart.state`. The driver suppresses nothing:
each phase prints its own `PASS`/`FAIL`/`BLOCKED` lines and the driver's exit
status is `0` only when every phase and every driver-side assertion passed.

The image does not include noVNC or x11vnc: the viewer needs one-time tokens
(`REQ-API-019`), which are not implemented, so a `MIEN_VIEWER_TOKEN` request is
refused by the entrypoint rather than served unauthenticated.

Exit codes: `0` all assertions passed, `1` at least one assertion failed,
`3` blocked before any assertion could run (for example an already-initialised
volume whose rotated password is unknown; pass `MIEN_VERIFY_PASSWORD` to supply
it, or start on a clean volume). A blocked run never reports `PASS`.

### Measured result

Run on a real `linux/amd64` host with Docker 29.9.3, image `mien-server:t30b`
(2.19 GB), clean volume, with the hardening from the run command above
(`--read-only`, `--cap-drop ALL`, `no-new-privileges`, the spike's seccomp
profile, `apparmor=unconfined`, uid `10001`):

```
RESULT PASS (0 failures)
isolation_actual=enforced
seccomp_user_namespace_descendants=2
start1 kernel_process pid=160 comm=chrome exe=/opt/mien/chromium/chrome
start2 kernel_process pid=265
no_root_processes_besides_init
schema_version: 8
headless_isolation_actual=enforced
headless kernel_pid=343 cmdline=/opt/mien/chromium/chrome ... --window-size=1280,720 --headless=new ... --mien-runtime-id=rt-...
restart pre_restart_state=running runtime_row_state=crashed api_state=crashed
restart orphan_kernel_processes: 0
hard_restart_orphan_kernel_processes: 0
hard_restart_reconcile_log: runtime reconciliation finished
```

`isolation=enforced` came from the API and was cross-checked against `/proc`:
two descendants of the kernel carried `Seccomp: 2` and lived in a user
namespace distinct from PID 1's. A run where those two disagree reports a
failure rather than downgrading the claim.

Restarting the container on the same volume kept the profile row, skipped
re-migration (`schema_migrations` still `8`) and served `200` on both
`/healthz` and `/readyz`. Both the graceful restart and the SIGKILL restart
settled the leftover runtime row to `crashed` with `pid`/`pgid` cleared and
`last_known_isolation=enforced` retained, and left zero kernel processes behind.

`apparmor=unconfined` above is the documented fallback, **not** a verified
hardened path: the `mien-chromium` profile has not been loaded on any available
host because `securityfs` cannot be mounted there. The acceptance run above
therefore says nothing about AppArmor; do not record it as verified.

The headless profile is proven headless from the kernel's own argv, not from the
API: `--headless=new` is an exact token in `/proc/<pid>/cmdline`, and
`--window-size=W,H` is the only window-shaped argument left, which is viewport
geometry rather than display evidence. `reduced_assurance=true` and
`direct=true` are printed as the API returns them, and `isolation` is printed
unmodified even when it is not `enforced`.

The build needs network access to Docker Hub for the base image. When a registry
returns `429`, point `BASE_IMAGE` at a mirror that serves the _same_ digest
(`--build-arg BASE_IMAGE=docker.m.daocloud.io/library/debian:bookworm-slim`,
which resolved to the pinned `sha256:e67ee…` above); the digest pin, not the
tag, is what the build verifies.
