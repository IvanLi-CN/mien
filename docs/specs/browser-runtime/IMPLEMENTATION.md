# 浏览器运行时实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 状态机、进程监督、Xvfb 与沙箱契约已建立。

## Implementation Coverage

- Requirement coverage: 计划由 `mien-core` 的 runtime 模块、代理网关、服务端宿主和 Docker 入口脚本覆盖 `REQ-RUNTIME-001` 至 `REQ-RUNTIME-019`；共享 Xray 引擎先于内核就绪，运行时停止只删除本档案入站/路由。
- Verification commands: 尚未建立；实现后使用生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱与 `isolation` 测试、共享引擎故障 teardown 测试和日志尾部测试。
- Rollout facts: 首个实现仅覆盖 macOS Apple Silicon 与 Docker `linux/amd64`；Windows/Linux 桌面后续接入；`runtimes.isolation` 与安装记录双哈希随首个迁移落地。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 进程启动、状态机、对账、代理引擎联动、CDP、Xvfb、openbox、noVNC、沙箱探测和日志轮转均未实现。
- 状态机仍缺内核退出（`starting|running -> stopping -> crashed`）与引擎故障/重建失败（`running -> stopping -> failed`）的显式处理，以及先 teardown 再写终态的保证。
- 启动仍缺“校验安装记录 `binary_sha256`”与 `KERNEL_NOT_FOUND`/`KERNEL_CHECKSUM_MISMATCH` 分流；沙箱校验尚未固定在创建代理资源和 spawn 之前。
- `runtimes.isolation`、停止时清空 `proxy_*` 并递减非终态引用、控制面重启时清理 `starting`/`stopping` 中间态，以及有界日志尾部端点均未实现。
- 需要先验证目标容器环境的命名空间沙箱可行性，再确定默认容器安全配置。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`；当前未实现项为 pending 并列出计划命令与 fixture，结构检查也接受有证据的已实施状态，发布门禁只接受 pass 或有效 waiver。

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-RUNTIME-001` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-002` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-003` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-004` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-005` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-006` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-007` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-008` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-009` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-010` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-011` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-012` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-013` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-014` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-015` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-016` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-017` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-018` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |
| `VER-RUNTIME-019` | `pending` | 生命周期集成测试、强杀对账测试、Docker 端到端测试、沙箱探针；fixture 根 `fixtures/runtime/` |

## M1 slice evidence

- Delivered: `RuntimeSupervisor` state machine (`starting/running/stopping/stopped/crashed/failed`), process groups, SIGTERM/SIGKILL shutdown, CDP probe, bounded logs, restart reconciliation, and `StartProfile` wiring profile → launch plan → proxy → supervisor.
- Commands: `cargo test -p mien-core` with `MIEN_KERNEL_BIN` pointing at the pinned macOS kernel; `tests/runtime_integration.rs` and `tests/slice_e2e.rs`.
- Measured: start/stop sequence emits each event once, no residual process group, CDP reachable while running and closed after stop, SIGKILL yields `crashed`, 20 cycles without orphans.
- Not delivered yet: headless/pseudo-headful and noVNC. Docker display and namespace sandbox are verified in `docs/spikes/docker/`; the product-level Linux probe (`probe_linux_isolation`) now fails closed unless a sandboxed descendant is observed. The AppArmor profile is authored but not yet loaded on a capable host.

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
