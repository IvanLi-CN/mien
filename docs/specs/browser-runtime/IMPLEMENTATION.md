# 浏览器运行时实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 部分实现（M2）：macOS 有头/无头/伪有头生命周期、进程组监督、CDP 探针、有界日志、崩溃退避与并发准入已落地；Docker 显示、noVNC、远程查看、服务端沙箱与持久化 `runtimes` 行未实现。
- Lifecycle: active
- Catalog note: 状态机、进程监督、Xvfb 与沙箱契约已建立。

## Implementation Coverage

- Requirement coverage: `mien-core` 的 runtime 模块覆盖 `REQ-RUNTIME-001` 至 `REQ-RUNTIME-019` 的桌面子集：`RuntimeSupervisor` 状态机（`starting/running/stopping/stopped/crashed/failed`）、进程组与 SIGTERM/SIGKILL、CDP 探测、有界 JSONL 日志、崩溃阶梯（1/2/4/8 s）、并发准入（默认 10）、重启对账，以及 `StartProfile` 的 profile → launch plan → proxy → supervisor 接线。
- Verification commands: 可复跑证据见 `## M2 slice evidence`；Docker、noVNC 与远程查看没有对应命令，相关 VER 保持 `pending`。
- Rollout facts: 首个实现仅覆盖 macOS Apple Silicon 与 Docker `linux/amd64`；Windows/Linux 桌面后续接入；`runtimes.isolation` 与安装记录双哈希随首个迁移落地。

## Coverage / rollout summary

- 真实内核实测：启动→running→停止→stopped 事件各一次，无残留进程组；SIGKILL 得 `crashed`；20 次循环无孤儿；10 档案并发峰值 RSS ≈1.4 GiB、第 11 个 `RUNTIME_CONFLICT`。
- `runtimes` 表虽有迁移（含 `isolation`/`last_known_isolation`），但仓库中没有任何写入或更新该表的代码；运行时状态只存在于内存与 `runtime.json`。

## Remaining Gaps

- 没有服务端：仓库只有 `mien-core` 与 `src-tauri`，无 `mien-server` crate；`/healthz`、`/readyz` 与 HTTP 日志尾部端点无从实现。
- Docker 显示（Xvfb/openbox）、`MIEN_SCREEN`、noVNC 与一次性查看器 URL 全部未实现。
- Linux 命名空间沙箱探测与 `ALLOW_UNSANDBOXED` 门禁未接线到启动路径，`isolation=enforced` 只在 macOS seatbelt 上成立。
- 运行时状态不落库，因此“最终状态与数据库一致”“重启后 `last_known_isolation` 仍保留”无法验证。
- 日志尾部只存在于监督器内存，没有 HTTP 端点，也没有“超限 `limit` 被拒绝”的校验。
- 头显/无头/伪有头的 UI 表述齐全，但“退出应用后进程被停止”只在测试内用 `stop()` 模拟，未验证真实应用退出路径。
- 崩溃阶梯是 1/2/4/8 s 共四次，SPEC 写的是“最多 3 次”；阶梯常量与 SPEC 尚未对齐。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`。本表的 `pass` 只表示“仓库内存在可运行命令且 T19 亲自运行通过”，并写出确切命令；其余保持 `pending` 并写明缺什么。发布门禁只接受 pass 或有效 waiver。

复跑环境（下文命令引用这两个变量）：

```sh
export KERNEL=/Users/ivan/orca/workspaces/mien/t4-kernel-spike/fixtures/.cache/kernel/148/macos-arm64/Chromium.app/Contents/MacOS/Chromium
export XRAY=/opt/homebrew/bin/xray
```

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-RUNTIME-001` | `pending` | `Command::args` 按数组传递、`env_clear` 后只注入 `MIEN_RUNTIME_ID`、`process_group(0)` 有实现（`--test runtime_integration`）;缺含空格/引号/Unicode 的路径用例与“数据目录唯一”的断言。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-002` | `pending` | `MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test runtime_integration --test reconcile`（2 + 6 passed）覆盖启动/停止/崩溃/控制面重启；缺取消、就绪前退出、pre-resource 与 post-resource `failed` 的完整事件序列，且无数据库一致性断言（运行时不落库）。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-003` | `pending` | 同 id 重复启动返回既有快照（`--lib runtime::supervisor`），重复停止在 `stopped` 上为空操作；但“并发 10 次启动请求只产生一个内核”没有用例（现有并发用例是 10 个不同 id）。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-004` | `pending` | `KERNEL_NOT_FOUND` 有单测与 IPC 测试（`--lib runtime::probe`、`src-tauri` 的 `runtime_start_with_a_missing_kernel_bin_fails_closed`）；`KERNEL_CHECKSUM_MISMATCH` 无任何用例，`kernel::registry::verify_binary` 仍是 `todo!()`。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-005` | `pass` | `MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test admission`（3 passed）：默认准入 10，第 11 个返回 `RUNTIME_CONFLICT` 且不建记录、不留进程，停止后槽位立即可用；10 个真实有头内核并发时峰值 RSS 断言 < 4 GiB，全部停止后残留进程为 0。受限内存/磁盘由 RSS 预算与本机实际占用间接覆盖，未使用 cgroup 限额。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-006` | `pending` | 真实内核在 macOS 上有头启动、CDP 可达、停止后窗口进程消失（`--test runtime_integration`、`src-tauri` 的 `ui_payloads_drive_the_real_kernel_lifecycle`）；未验证真实窗口出现，也未验证应用退出路径。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-007` | `pending` | Docker 有头显示、Xvfb/openbox 与 `MIEN_SCREEN` 在代码与测试中都不存在。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-008` | `pending` | noVNC 查看器、一次性 URL 与令牌过期均未实现。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-009` | `pass` | `MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test mode_matrix`（4 passed）与 `bun run test`（21 passed）：三种模式各自启动，`--headless=new` 只出现在无头/伪有头，真实进程命令行与计划 argv 一致，有头请求不会被改写成无头，响应与事件带 `reduced_assurance` 与模式声明，UI 卡片渲染这些 disclosure。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-010` | `pending` | 崩溃经 `stopping` 写 `crashed`、默认不重启、阶梯耗尽写 `failed`、显式停止优先均有证据（`--test reconcile`、`--lib runtime::admission`）；但阶梯是四次而非 SPEC 写的“最多 3 次”，“稳定运行后计数重置”没有用例，teardown 失败保留 `stopping` 也无断言。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-011` | `pending` | 停止优先优雅退出、超时后整组终止有 `stop_process`；但入站/规则按 `ruleTag` 删除只在代理侧，`proxy_*` 同事务清空未实现，审计事件与 `ref_count` 不漂移均无断言。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-012` | `pending` | `MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test reconcile` 覆盖控制面重启接管、可归属残留清理、外来进程不误杀；缺 `starting`/`stopping` 中段强杀、PID 复用、“无法确认退出时保留端口与 `stopping`”的用例。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-013` | `pending` | 回环 CDP 可达、关闭自动化时不建调试端口、代理档案等入站就绪均有实现；缺容器外/局域网不可连的断言，也没有 `--remote-debugging-address` 绑定回环的显式配置。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-014` | `pending` | macOS 有 `probe_macos_sandbox` 且先于 spawn；服务端 `ALLOW_UNSANDBOXED`、Linux 命名空间启动前校验与“未创建出站/入站/规则”的顺序断言都未实现。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-015` | `pending` | JSONL 行、脱敏（Cookie/token/URL）与有界尾部有单测（`--lib runtime::logs`）；日志轮转常量存在但无测试，`GET /api/v1/profiles/{id}/logs` 端点不存在。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-016` | `pending` | 无服务端与 `/healthz`、`/readyz`。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-017` | `pass` | `MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test mode_matrix`（4 passed）与 `bun run test`（21 passed）：声明 `headless_chrome_ua_patch` 时 `pseudo_headful` 启动并返回 `ua_scope=chrome`；去掉该能力后以 `RUNTIME_MODE_UNSUPPORTED` 在 spawn 前拒绝、不建记录不发事件，不静默降级；其他 Headless 特征说明仍在 disclosures 中并被 UI 渲染。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-018` | `pending` | `reduced_assurance` 真值表 18 组合全枚举（`--test mode_matrix`），UI 有 RT/IS proof strip 单测（`bun run test`）；但运行时不落库，`last_known_isolation` 的“仅 `running` 事务更新、重启后保留”无法验证。fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-019` | `pending` | 引擎崩溃后绑定保留、重建成功恢复、阶梯耗尽写 `failed` 有引擎层证据（`--lib proxy::engine`）；运行时层“保留 `running` 与原端口、清理未完成保留 `stopping`、已终态不复活”的端到端断言缺失。fixture 根 `fixtures/runtime/` |

## M1 slice evidence

- Delivered: `RuntimeSupervisor` state machine (`starting/running/stopping/stopped/crashed/failed`), process groups, SIGTERM/SIGKILL shutdown, CDP probe, bounded logs, restart reconciliation, and `StartProfile` wiring profile → launch plan → proxy → supervisor.
- Commands: `cargo test -p mien-core` with `MIEN_KERNEL_BIN` pointing at the pinned macOS kernel; `tests/runtime_integration.rs` and `tests/slice_e2e.rs`.
- Measured: start/stop sequence emits each event once, no residual process group, CDP reachable while running and closed after stop, SIGKILL yields `crashed`, 20 cycles without orphans.
- Not delivered yet: headless/pseudo-headful and noVNC. Docker display and namespace sandbox are verified in `docs/spikes/docker/`; the product-level Linux probe (`probe_linux_isolation`) now fails closed unless a sandboxed descendant is observed. The AppArmor profile is authored but not yet loaded on a capable host.

## M2 slice evidence

- Re-run in T19 on 2026-11-27, macOS 27.2 / arm64:
  - `MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test runtime_integration --test mode_matrix --test reconcile --test admission --test network_canary` — 2 + 4 + 6 + 3 + 5 passed。
  - `cargo test --manifest-path src-tauri/Cargo.toml --locked` — 1 + 1 + 7 + 5 passed。
  - `bun run test` — 21 passed（含六态状态标签与 RT/IS proof strip）。
- `VER-RUNTIME-005`, `VER-RUNTIME-009` and `VER-RUNTIME-017` are the only rows whose pass condition is fully covered by these runs.

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)

最近同步：M2（T19），依据 `MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test runtime_integration --test mode_matrix --test reconcile --test admission --test network_canary`、`cargo test --manifest-path src-tauri/Cargo.toml --locked`、`bun run test`、`cargo run -q -p xtask -- check --skip-host`。
