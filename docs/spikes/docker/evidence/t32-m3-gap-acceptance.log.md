# T32 M3 gap acceptance: recovered raw output

The raw logs from the T32 acceptance run were deleted from the Docker host during cleanup, together with `/tmp/mien-*.out`. What follows is recovered from the agent session transcript, which stores each tool result as it was received. Nothing here was retyped from memory: every block is a byte-for-byte copy of a transcript record, including the harness's own trailing `Wall time` line, and the exact command that produced it is printed directly above it.

## What is lost and what survives

**The complete 91-line output of the final run is lost.** It was redirected to `/tmp/t32-acceptance-final.log` on the host and only `grep` results from it were ever printed, so records `578`, `581`, and `589` below are all that remain of that run. No complete log exists for any run in this task: the two runs whose stdout was streamed were both piped through `tail`, and the third was redirected to a host file and then filtered.

Six first-hand host outputs survive:

| Record | Captured | Command shape | Lines | `PASS` lines |
| --- | --- | --- | --- | --- |
| `392` | 17:36:58Z | `verify-restart.sh … \| tail -80` | 83 | 55 |
| `486` | 17:47:53Z | `verify-restart.sh … \| tail -70` | 75 | 48 |
| `500` | 17:49:24Z | `sed -n 1,20p` + `grep` of a host log | 32 | 12 |
| `578` | 17:58:00Z | counts + `RESULT` grep of the final log | 10 | 0 |
| `581` | 17:58:15Z | evidence grep of the final log | 14 | 0 |
| `589` | 17:58:37Z | `grep -n` section map of the final log | 35 | 0 (11 numbered) |

The 91 PASS / 0 FAIL / 0 BLOCKED claim rests on record `578`: `grep -c '^PASS'` printed `91` and `grep -E '^(FAIL|BLOCKED)'` printed nothing, with `exit=0`. The line-by-line listing behind that count no longer exists; this file cannot be used to reconstruct it.

Records `392`, `486`, and `500` are `tail` or `head` windows of live runs. The three together cover the headless phase, both restarts, and the in-container head, but no single window shows all 91 assertions.

## Script hashes at the time of the final run

| File | SHA-256 |
| --- | --- |
| `deploy/docker/verify-server.sh` | `fcc3ef384374e356de27420da0a11ee7cdeea7f632bbf66c874a36e95c53e322` |
| `deploy/docker/verify-restart.sh` | `634598193446f10929cbef9e0e26f93d5f1f281df1126d89ccd1541fdc9b05dd` |

The `verify-server.sh` hash was printed from inside the build context by the same command that ran the final acceptance, and the same value was computed over the committed file (`31c5f31`). The bytes that ran are the bytes committed.

Between the 17:47:53Z window and the final run there was exactly one script edit, at 17:53:43Z, and it changed comment lines only (0 non-comment changed lines, checked against the recorded diff). The assertion logic behind both windows is therefore identical.

## Final run: total counts

Command:

```sh
ssh -o ConnectTimeout=10 192.168.31.11 'sha256sum ~/mien-t32-build/context/verify-server.sh; cd ~/mien-t32-build/repo && ./deploy/docker/verify-restart.sh mien-server:t32 > /tmp/t32-acceptance-final.log 2>&1; echo "exit=$?"; grep -c "^PASS" /tmp/t32-acceptance-final.log; grep -E "^(FAIL|BLOCKED)" /tmp/t32-acceptance-final.log | head; grep -E "^RESULT" /tmp/t32-acceptance-final.log'
```

Output:

```
fcc3ef384374e356de27420da0a11ee7cdeea7f632bbf66c874a36e95c53e322  /home/ivan/mien-t32-build/context/verify-server.sh
exit=0
91
RESULT PASS (0 failures)
RESULT PASS (0 failures)
RESULT PASS (0 failures)
RESULT PASS (0 failures)


Wall time: 50.12 seconds
```

The four `RESULT PASS (0 failures)` lines are the in-container `full` phase, the graceful-restart `check`, the hard-restart `check`, and the driver summary.

## Final run: selected assertion and evidence lines

Command:

```sh
shasum -a 256 deploy/docker/verify-server.sh; ssh -o ConnectTimeout=10 192.168.31.11 'grep -E "^(run_command|headless kernel_pid|headless_isolation|restart pre_restart|restart orphan|hard_restart_orphan|hard_restart_reconcile_log_line|seccomp_user|start1 kernel|start2 kernel|schema_version|no_root|isolation_actual)" /tmp/t32-acceptance-final.log; echo "=== image:"; docker image inspect mien-server:t32 --format "size_bytes={{.Size}}"; docker images mien-server:t32 --format "{{.Repository}}:{{.Tag}} {{.Size}}"'
```

Output:

```
fcc3ef384374e356de27420da0a11ee7cdeea7f632bbf66c874a36e95c53e322  deploy/docker/verify-server.sh
run_command=docker run -d --name mien-restart-acceptance --read-only --tmpfs /tmp:rw,noexec,nosuid,size=1g --shm-size=1g --cap-drop ALL --security-opt no-new-privileges --security-opt seccomp=/home/ivan/mien-t32-build/repo/deploy/docker/seccomp/chromium.json --security-opt apparmor=unconfined --user 10001:10001 --pids-limit 512 --memory 4g --cpus 2 --ulimit nofile=4096:4096 -p 127.0.0.1:8787:8787 -v mien-restart-acceptance-data:/data -e MIEN_BIND=0.0.0.0:8787 -e MIEN_PUBLIC_URL=http://127.0.0.1:8787 -e MIEN_ALLOW_DIRECT_EGRESS=1 mien-server:t32
isolation_actual=enforced
seccomp_user_namespace_descendants=2
headless_isolation_actual=enforced
headless kernel_pid=394 cmdline=/opt/mien/chromium/chrome --fingerprint=1651157675 --fingerprint-platform=linux --fingerprint-platform-version=6.8.0 --fingerprint-brand=Edge --fingerprint-brand-version=148.0.0.0 --fingerprint-hardware-concurrency=12 --lang=en-US --accept-lang=en-US,en --timezone=UTC --window-size=1280,720 --headless=new --user-data-dir=/data/profiles/01M8F2F93G000000000000800X/chrome --mien-runtime-id=rt-01M8F2F93G000000000000800X --use-mock-keychain --password-store=basic --no-first-run --no-default-browser-check 
restart pre_restart_state=running runtime_row_state=crashed api_state=crashed
hard_restart_reconcile_log_line=2026-11-27T17:57:58.245370Z  INFO mien_server::profiles: runtime reconciliation finished count=1
restart pre_restart_state=running runtime_row_state=crashed api_state=crashed
=== image:
size_bytes=2189939139
mien-server:t32 2.19GB
[output summarized by the chain filter; pipe through `| cat` for verbatim output]
[raw output: local://3.log]
```

## Final run: section map with line numbers

Command:

```sh
ssh -o ConnectTimeout=10 192.168.31.11 'grep -nE "^(PASS (start1 kernel|start2 kernel|no_root|schema_version|restart orphan|hard_restart_orphan|hard_restart_reconcile_log|graceful_restart)|RESULT|== )" /tmp/t32-acceptance-final.log'
```

Output (line numbers refer to `/tmp/t32-acceptance-final.log`, which is gone; the `grep -n` prefixes survive, the bodies do not):

```
1:== start container ==
6:== in-container acceptance (phase full) ==
7:== health ==
13:== auth ==
22:== profile ==
26:== runtime start #1 ==
33:PASS start1 kernel_process pid=142 comm=chrome exe=/opt/mien/chromium/chrome
36:== runtime stop #1 ==
40:== runtime start #2 ==
44:PASS start2 kernel_process pid=272
45:== runtime stop #2 (final) ==
48:== privileges ==
49:PASS no_root_processes_besides_init
51:== data root ==
57:== migrations ==
58:PASS schema_version: 8
59:== headless mode ==
77:== restart prepare ==
82:== summary ==
84:RESULT PASS (0 failures)
86:== graceful restart (docker restart) ==
87:PASS graceful_restart_issued
88:PASS graceful_restart ready: healthz 200, readyz 200
89:== restart check ==
101:PASS restart orphan_kernel_processes: 0
106:RESULT PASS (0 failures)
107:PASS graceful_restart_verify: exit 0
108:== hard kill (docker kill, SIGKILL) then start ==
113:PASS hard_restart_orphan_kernel_processes: 0
114:PASS hard_restart_reconcile_log: runtime reconciliation finished
116:== restart check ==
128:PASS restart orphan_kernel_processes: 0
129:RESULT PASS (0 failures)
131:== summary ==
133:RESULT PASS (0 failures)
```

## Window 17:49:24Z: container head plus selected evidence

Command:

```sh
ssh -o ConnectTimeout=10 192.168.31.11 'sed -n "1,20p" /tmp/t32-acceptance.log; echo ...; grep -E "^(run_command|image=|RESULT|start1 kernel_process|start2 kernel_process|no_root|schema_version|isolation_actual|seccomp_user|headless kernel_pid|headless_isolation)" /tmp/t32-acceptance.log'
```

Output (the `...` is the command's own separator, not an elision of the log):

```
== start container ==
image=mien-server:t32
run_command=docker run -d --name mien-restart-acceptance --read-only --tmpfs /tmp:rw,noexec,nosuid,size=1g --shm-size=1g --cap-drop ALL --security-opt no-new-privileges --security-opt seccomp=/home/ivan/mien-t32-build/repo/deploy/docker/seccomp/chromium.json --security-opt apparmor=unconfined --user 10001:10001 --pids-limit 512 --memory 4g --cpus 2 --ulimit nofile=4096:4096 -p 127.0.0.1:8787:8787 -v mien-restart-acceptance-data:/data -e MIEN_BIND=0.0.0.0:8787 -e MIEN_PUBLIC_URL=http://127.0.0.1:8787 -e MIEN_ALLOW_DIRECT_EGRESS=1 mien-server:t32
PASS container_started name=mien-restart-acceptance
PASS initial ready: healthz 200, readyz 200
== in-container acceptance (phase full) ==
== health ==
PASS healthz_status: 200
PASS readyz_status: 200 (ready)
readyz_body={"status":"ok","checks":{"database":{"ok":true},"kernel":{"ok":true}}}
PASS ui_index_status: 200
PASS ui_index_contains_mount_point
== auth ==
PASS bootstrap_secret_mode: 600
bootstrap_source=generated_file
PASS login_status: 200
PASS session_cookie_and_csrf_received cookie_len=64 csrf_len=64
PASS login_user: admin
PASS must_change_password: true
PASS password_change_status: 204
...
image=mien-server:t32
run_command=docker run -d --name mien-restart-acceptance --read-only --tmpfs /tmp:rw,noexec,nosuid,size=1g --shm-size=1g --cap-drop ALL --security-opt no-new-privileges --security-opt seccomp=/home/ivan/mien-t32-build/repo/deploy/docker/seccomp/chromium.json --security-opt apparmor=unconfined --user 10001:10001 --pids-limit 512 --memory 4g --cpus 2 --ulimit nofile=4096:4096 -p 127.0.0.1:8787:8787 -v mien-restart-acceptance-data:/data -e MIEN_BIND=0.0.0.0:8787 -e MIEN_PUBLIC_URL=http://127.0.0.1:8787 -e MIEN_ALLOW_DIRECT_EGRESS=1 mien-server:t32
isolation_actual=enforced
seccomp_user_namespace_descendants=2
headless_isolation_actual=enforced
headless kernel_pid=335 cmdline=/opt/mien/chromium/chrome --fingerprint=1784934879 --fingerprint-platform=linux --fingerprint-platform-version=6.8.0 --fingerprint-brand=Edge --fingerprint-brand-version=148.0.0.0 --fingerprint-hardware-concurrency=12 --lang=en-US --accept-lang=en-US,en --timezone=UTC --window-size=1280,720 --headless=new --user-data-dir=/data/profiles/01M8F1Z504000000000000800X/chrome --mien-runtime-id=rt-01M8F1Z504000000000000800X --use-mock-keychain --password-store=basic --no-first-run --no-default-browser-check 
RESULT PASS (0 failures)
RESULT PASS (0 failures)
RESULT PASS (0 failures)
image=mien-server:t32
RESULT PASS (0 failures)
```

## Window 17:47:53Z: `tail -70`, 48 PASS lines

The richest surviving window. It starts mid-flow at the headless phase because `tail` cut the earlier sections. This transcript record matches the retained spill file (`local/2.log`) byte-for-byte; its trailing `[output summarized by the builtin filter]` notice is the harness describing its own display step, not extra log content.

Command:

```sh
ssh -o ConnectTimeout=10 192.168.31.11 'cd ~/mien-t32-build/repo && ./deploy/docker/verify-restart.sh mien-server:t32 2>&1 | tail -70'
```

Output:

```
headless_isolation_actual=enforced
PASS headless reduced_assurance: true
PASS headless direct: true
PASS headless mode_reported: headless
headless limitations_actual=Host-bound screen size, devicePixelRatio, color depth and font enumeration remain observable.; Headless pseudo_headful mode changes UA only; other Headless characteristics remain.; Canvas sub-region readback can differ from full-surface readback (#98); WebGL readPixels can differ between 1x1 and 8x8 reads (#99); probe reports must attribute both gaps to the kernel, not to the spec.
headless kernel_pid=426 cmdline=/opt/mien/chromium/chrome --fingerprint=2977789930 --fingerprint-platform=linux --fingerprint-platform-version=6.8.0 --fingerprint-brand=Edge --fingerprint-brand-version=148.0.0.0 --fingerprint-hardware-concurrency=12 --lang=en-US --accept-lang=en-US,en --timezone=UTC --window-size=1280,720 --headless=new --user-data-dir=/data/profiles/01M8F1WN4D000000000000800X/chrome --mien-runtime-id=rt-01M8F1WN4D000000000000800X --use-mock-keychain --password-store=basic --no-first-run --no-default-browser-check 
PASS headless argv_contains --headless=new
PASS headless argv_has_no_headful_window_flags
headless window_size_arg=--window-size=1280,720
PASS headless argv_has_no_other_window_arguments
PASS headless stop_status: 202
PASS headless runtime_state: stopped
PASS headless kernel_process_gone pid=426
== restart prepare ==
PASS restart-prepare start_status: 202
PASS restart-prepare runtime_state: running
PASS restart_prepare runtime_row: running
restart_prepare state_file=/data/verify-restart.state profile_id=01M8F1W9KY000000000000400X isolation=enforced
== summary ==
profile_id=01M8F1W9KY000000000000400X isolation_last=enforced
RESULT PASS (0 failures)
PASS verify_phase_full: exit 0
== graceful restart (docker restart) ==
PASS graceful_restart_issued
PASS graceful_restart ready: healthz 200, readyz 200
== restart check ==
PASS restart healthz_status: 200
PASS restart readyz_status: 200
PASS restart probe_was_running: running
PASS restart login_status: 200
PASS restart session_cookie_and_csrf_received cookie_len=64 csrf_len=64
PASS restart profile_get_status: 200
PASS restart profile_id: 01M8F1W9KY000000000000400X
restart pre_restart_state=running runtime_row_state=crashed api_state=crashed
PASS restart runtime_row_terminal: crashed
PASS restart api_state_terminal: crashed
PASS restart last_known_isolation_retained: enforced
PASS restart orphan_kernel_processes: 0
PASS restart-rearm start_status: 202
PASS restart-rearm runtime_state: running
PASS restart rearm runtime_row: running
PASS restart rearm state_recorded profile_id=01M8F1W9KY000000000000400X
RESULT PASS (0 failures)
PASS graceful_restart_verify: exit 0
== hard kill (docker kill, SIGKILL) then start ==
PASS hard_kill_issued signal=SIGKILL
PASS hard_kill_restart_issued
PASS hard_restart ready: healthz 200, readyz 200
orphan_count=0
PASS hard_restart_orphan_kernel_processes: 0
PASS hard_restart_reconcile_log: runtime reconciliation finished
hard_restart_reconcile_log_line=2026-11-27T17:47:52.027235Z  INFO mien_server::profiles: runtime reconciliation finished count=1
== restart check ==
PASS restart healthz_status: 200
PASS restart readyz_status: 200
PASS restart probe_was_running: running
PASS restart login_status: 200
PASS restart session_cookie_and_csrf_received cookie_len=64 csrf_len=64
PASS restart profile_get_status: 200
PASS restart profile_id: 01M8F1W9KY000000000000400X
restart pre_restart_state=running runtime_row_state=crashed api_state=crashed
PASS restart runtime_row_terminal: crashed
PASS restart api_state_terminal: crashed
PASS restart last_known_isolation_retained: enforced
PASS restart orphan_kernel_processes: 0
RESULT PASS (0 failures)
PASS hard_restart_verify: exit 0
== summary ==
image=mien-server:t32
RESULT PASS (0 failures)
[output summarized by the builtin filter; pipe through `| cat` for verbatim output]
[raw output: local://2.log]


Wall time: 54.88 seconds
```

## Window 17:36:58Z: `tail -80`, 55 PASS lines

The earliest run in which the headless phase and both restarts appear. Its `kernel_alive` still used an unguarded redirect, which is why two `cannot open /proc/<pid>/cmdline: No such file` lines appear: dash reports the failed `<` redirect itself, and that noise is what the later guard removed. Both lines are reproduced as emitted.

Command:

```sh
ssh 192.168.31.11 'cd ~/mien-t32-build/repo && ./deploy/docker/verify-restart.sh mien-server:t32 2>&1 | tail -80'
```

Output:

```
PASS data_permissions /data/profiles mode=700 owner=10001:10001
PASS master_key_mode: 600
PASS master_key_owner: 10001:10001
== migrations ==
PASS schema_version: 8
== headless mode ==
PASS headless profile_create_status: 201 id=01M8F18MSE000000000000800X
PASS headless profile_mode: headless
PASS headless start_status: 202
PASS headless runtime_state: running
headless_isolation_actual=enforced
PASS headless reduced_assurance: true
PASS headless direct: true
PASS headless mode_reported: headless
headless limitations_actual=Host-bound screen size, devicePixelRatio, color depth and font enumeration remain observable.; Headless pseudo_headful mode changes UA only; other Headless characteristics remain.; Canvas sub-region readback can differ from full-surface readback (#98); WebGL readPixels can differ between 1x1 and 8x8 reads (#99); probe reports must attribute both gaps to the kernel, not to the spec.
headless kernel_pid=343 cmdline=/opt/mien/chromium/chrome --fingerprint=2601426313 --fingerprint-platform=linux --fingerprint-platform-version=6.8.0 --fingerprint-brand=Edge --fingerprint-brand-version=148.0.0.0 --fingerprint-hardware-concurrency=12 --lang=en-US --accept-lang=en-US,en --timezone=UTC --window-size=1280,720 --headless=new --user-data-dir=/data/profiles/01M8F18MSE000000000000800X/chrome --mien-runtime-id=rt-01M8F18MSE000000000000800X --use-mock-keychain --password-store=basic --no-first-run --no-default-browser-check 
PASS headless argv_contains --headless=new
PASS headless argv_has_no_headful_window_flags
headless window_size_arg=--window-size=1280,720
PASS headless argv_has_no_other_window_arguments
PASS headless stop_status: 202
PASS headless runtime_state: stopped
/usr/local/bin/verify-server.sh: 165: cannot open /proc/343/cmdline: No such file
/usr/local/bin/verify-server.sh: 165: cannot open /proc/343/cmdline: No such file
PASS headless kernel_process_gone pid=343
== restart prepare ==
PASS restart-prepare start_status: 202
PASS restart-prepare runtime_state: running
PASS restart_prepare runtime_row: running
restart_prepare state_file=/data/verify-restart.state profile_id=01M8F186SW000000000000400X isolation=enforced
== summary ==
profile_id=01M8F186SW000000000000400X isolation_last=enforced
RESULT PASS (0 failures)
PASS verify_phase_full: exit 0
== graceful restart (docker restart) ==
PASS graceful_restart_issued
PASS graceful_restart ready: healthz 200, readyz 200
== restart check ==
PASS restart healthz_status: 200
PASS restart readyz_status: 200
PASS restart probe_was_running: running
PASS restart login_status: 200
PASS restart session_cookie_and_csrf_received cookie_len=64 csrf_len=64
PASS restart profile_get_status: 200
PASS restart profile_id: 01M8F186SW000000000000400X
restart pre_restart_state=running runtime_row_state=crashed api_state=crashed
PASS restart runtime_row_terminal: crashed
PASS restart api_state_terminal: crashed
PASS restart last_known_isolation_retained: enforced
PASS restart orphan_kernel_processes: 0
PASS restart-rearm start_status: 202
PASS restart-rearm runtime_state: running
PASS restart rearm runtime_row: running
PASS restart rearm state_recorded profile_id=01M8F186SW000000000000400X
RESULT PASS (0 failures)
PASS graceful_restart_verify: exit 0
== hard kill (docker kill, SIGKILL) then start ==
PASS hard_kill_issued signal=SIGKILL
PASS hard_kill_restart_issued
PASS hard_restart ready: healthz 200, readyz 200
orphan_count=0
PASS hard_restart_orphan_kernel_processes: 0
== restart check ==
PASS restart healthz_status: 200
PASS restart readyz_status: 200
PASS restart probe_was_running: running
PASS restart login_status: 200
PASS restart session_cookie_and_csrf_received cookie_len=64 csrf_len=64
PASS restart profile_get_status: 200
PASS restart profile_id: 01M8F186SW000000000000400X
restart pre_restart_state=running runtime_row_state=crashed api_state=crashed
PASS restart runtime_row_terminal: crashed
PASS restart api_state_terminal: crashed
PASS restart last_known_isolation_retained: enforced
PASS restart orphan_kernel_processes: 0
RESULT PASS (0 failures)
PASS hard_restart_verify: exit 0
== summary ==
image=mien-server:t32
RESULT PASS (0 failures)


Wall time: 57.59 seconds
```

## Headless process command line, verbatim

Four independent launches, each read from `/proc/<pid>/cmdline` by `verify-server.sh` inside the container. These are the original `say` lines, unmodified; the command-line field of each is 505 characters and ends with the trailing space that `tr '\0' ' '` produced for the NUL terminating the final argv element.

```
headless kernel_pid=343 cmdline=/opt/mien/chromium/chrome --fingerprint=2601426313 --fingerprint-platform=linux --fingerprint-platform-version=6.8.0 --fingerprint-brand=Edge --fingerprint-brand-version=148.0.0.0 --fingerprint-hardware-concurrency=12 --lang=en-US --accept-lang=en-US,en --timezone=UTC --window-size=1280,720 --headless=new --user-data-dir=/data/profiles/01M8F18MSE000000000000800X/chrome --mien-runtime-id=rt-01M8F18MSE000000000000800X --use-mock-keychain --password-store=basic --no-first-run --no-default-browser-check 
headless kernel_pid=426 cmdline=/opt/mien/chromium/chrome --fingerprint=2977789930 --fingerprint-platform=linux --fingerprint-platform-version=6.8.0 --fingerprint-brand=Edge --fingerprint-brand-version=148.0.0.0 --fingerprint-hardware-concurrency=12 --lang=en-US --accept-lang=en-US,en --timezone=UTC --window-size=1280,720 --headless=new --user-data-dir=/data/profiles/01M8F1WN4D000000000000800X/chrome --mien-runtime-id=rt-01M8F1WN4D000000000000800X --use-mock-keychain --password-store=basic --no-first-run --no-default-browser-check 
headless kernel_pid=335 cmdline=/opt/mien/chromium/chrome --fingerprint=1784934879 --fingerprint-platform=linux --fingerprint-platform-version=6.8.0 --fingerprint-brand=Edge --fingerprint-brand-version=148.0.0.0 --fingerprint-hardware-concurrency=12 --lang=en-US --accept-lang=en-US,en --timezone=UTC --window-size=1280,720 --headless=new --user-data-dir=/data/profiles/01M8F1Z504000000000000800X/chrome --mien-runtime-id=rt-01M8F1Z504000000000000800X --use-mock-keychain --password-store=basic --no-first-run --no-default-browser-check 
headless kernel_pid=394 cmdline=/opt/mien/chromium/chrome --fingerprint=1651157675 --fingerprint-platform=linux --fingerprint-platform-version=6.8.0 --fingerprint-brand=Edge --fingerprint-brand-version=148.0.0.0 --fingerprint-hardware-concurrency=12 --lang=en-US --accept-lang=en-US,en --timezone=UTC --window-size=1280,720 --headless=new --user-data-dir=/data/profiles/01M8F2F93G000000000000800X/chrome --mien-runtime-id=rt-01M8F2F93G000000000000800X --use-mock-keychain --password-store=basic --no-first-run --no-default-browser-check 
```

`pid=394` belongs to the final run. `--headless=new` is a standalone argv element in all four. The script does not accept a substring hit: it splits the NUL-separated `/proc/<pid>/cmdline` into one token per line and matches with `grep -qx -- '--headless=new'`. The only window-shaped argument present is `--window-size=1280,720`, which headful launches carry as well; `--start-maximized`, `--start-fullscreen`, and `--kiosk` are separately asserted absent, as are `--app=`, `--new-window`, `--window-position=`, `--window-name=`, and `--auto-open-devtools-for-tabs=`.

## Restart reconciliation: measured states

From the final-run greps:

```
restart pre_restart_state=running runtime_row_state=crashed api_state=crashed
hard_restart_reconcile_log_line=2026-11-27T17:57:58.245370Z  INFO mien_server::profiles: runtime reconciliation finished count=1
```

Both restart phases observed `crashed`, and the phase accepts `stopped`, `crashed`, or `failed` (`case "$row_state" in stopped | crashed | failed`), so the assertion is "terminal", not "terminal and specifically crashed".

## Assertion coverage across the surviving first-hand output

Every distinct `PASS` label seen in records 392, 486, 500, 578, 581, and 589, deduplicated so a re-read of the same output cannot inflate a count. Per-run identifiers are folded to `N`/`X`. This is the honest substitute for a complete listing: it shows which assertions were observed to pass, not that one run printed all 91.

| Times observed | Assertion |
| --- | --- |
| 6 | `restart orphan_kernel_processes: 0` |
| 4 | `restart api_state_terminal: crashed` |
| 4 | `restart healthz_status: 200` |
| 4 | `restart last_known_isolation_retained: enforced` |
| 4 | `restart login_status: 200` |
| 4 | `restart probe_was_running: running` |
| 4 | `restart profile_get_status: 200` |
| 4 | `restart readyz_status: 200` |
| 4 | `restart runtime_row_terminal: crashed` |
| 4 | `restart session_cookie_and_csrf_received cookie_len=64 csrf_len=64` |
| 3 | `graceful_restart ready: healthz 200, readyz 200` |
| 3 | `graceful_restart_issued` |
| 3 | `graceful_restart_verify: exit 0` |
| 3 | `hard_restart_orphan_kernel_processes: 0` |
| 2 | `hard_kill_issued signal=SIGKILL` |
| 2 | `hard_kill_restart_issued` |
| 2 | `hard_restart ready: healthz 200, readyz 200` |
| 2 | `hard_restart_reconcile_log: runtime reconciliation finished` |
| 2 | `hard_restart_verify: exit 0` |
| 2 | `headless argv_contains --headless=new` |
| 2 | `headless argv_has_no_headful_window_flags` |
| 2 | `headless argv_has_no_other_window_arguments` |
| 2 | `headless direct: true` |
| 2 | `headless kernel_process_gone pid=N` |
| 2 | `headless mode_reported: headless` |
| 2 | `headless reduced_assurance: true` |
| 2 | `headless runtime_state: stopped` |
| 2 | `headless stop_status: 202` |
| 2 | `restart profile_id: 01M8F186SW000000000000400X` |
| 2 | `restart profile_id: 01M8F1W9KY000000000000400X` |
| 2 | `restart rearm runtime_row: running` |
| 2 | `restart rearm state_recorded profile_id=X` |
| 2 | `restart-prepare runtime_state: running` |
| 2 | `restart-prepare start_status: 202` |
| 2 | `restart-rearm runtime_state: running` |
| 2 | `restart-rearm start_status: 202` |
| 2 | `restart_prepare runtime_row: running` |
| 2 | `schema_version: 8` |
| 2 | `verify_phase_full: exit 0` |
| 1 | `bootstrap_secret_mode: 600` |
| 1 | `container_started name=mien-restart-acceptance` |
| 1 | `data_permissions /data/profiles mode=700 owner=10001:10001` |
| 1 | `headless profile_create_status: 201 id=X` |
| 1 | `headless profile_mode: headless` |
| 1 | `headless runtime_state: running` |
| 1 | `headless start_status: 202` |
| 1 | `healthz_status: 200` |
| 1 | `initial ready: healthz 200, readyz 200` |
| 1 | `login_status: 200` |
| 1 | `login_user: admin` |
| 1 | `master_key_mode: 600` |
| 1 | `master_key_owner: 10001:10001` |
| 1 | `must_change_password: true` |
| 1 | `no_root_processes_besides_init` |
| 1 | `password_change_status: 204` |
| 1 | `readyz_status: 200 (ready)` |
| 1 | `session_cookie_and_csrf_received cookie_len=64 csrf_len=64` |
| 1 | `start1 kernel_process pid=N comm=chrome exe=/opt/mien/chromium/chrome` |
| 1 | `start2 kernel_process pid=N` |
| 1 | `ui_index_contains_mount_point` |
| 1 | `ui_index_status: 200` |
