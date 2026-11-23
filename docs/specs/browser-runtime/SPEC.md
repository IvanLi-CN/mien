# 浏览器运行时与显示环境

> 本文件是运行时主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: 产品必须可靠地启动、监督和停止 fingerprint-chromium 进程，并在 macOS 宿主桌面与 Docker 无桌面环境中提供有头和无头两种运行方式。
- In scope: 运行时状态机、启动/停止、进程组、崩溃处理、控制面重启对账、并发准入、内核校验、沙箱检查、CDP 暴露、日志、Xvfb/openbox/noVNC 生命周期。
- Out of scope: 指纹字段与 argv 语义（由 `fingerprint-engine` 负责）、代理协议与 DNS（由 `proxy-gateway` 负责）、档案元数据（由 `profile-management` 负责）。

## Terms and Interfaces

- `runtime`: 一个受监督的内核进程实例及其协议引擎、调试端口、日志和显示环境。
- `runtime state`: `stopped`、`starting`、`running`、`stopping`、`crashed`、`failed`。
- `process group`: 内核及其子进程组成的进程组，用于可靠终止和孤儿清理。
- `headful`: 内核创建可见窗口；macOS 使用宿主桌面，Docker 使用 Xvfb。
- `headless`: 内核不创建可见窗口；标记为降低保证。
- Interface: `/api/v1/profiles/{id}/start`、`/stop`、`/runtime`；运行时事件 `runtime.starting`、`runtime.running`、`runtime.stopping`、`runtime.stopped`、`runtime.crashed`、`runtime.failed`；`runtimes` 表。

## Requirements

### REQ-RUNTIME-001

- The system MUST spawn the kernel as a direct child process using an argument array, a dedicated user-data directory, and a new process group; it MUST NOT invoke a shell.
- Inputs: 内核可执行路径、指纹 argv、档案数据目录、协议引擎 mixed 入口、运行模式。
- Outputs: 内核 PID 与进程组 ID；子进程环境变量最小化；任何启动参数不得来自未校验的字符串拼接。

### REQ-RUNTIME-002

- The system MUST enforce the runtime state machine and MUST emit a state event for every transition.
- Inputs: 启动、停止、内核退出、超时、控制面重启。
- Outputs: 合法转移序列；非法转移必须被拒绝并记录；外部观察者可以通过事件流重建状态。

### REQ-RUNTIME-003

- The system MUST make start and stop idempotent per profile and MUST allow at most one supervised runtime per profile.
- Inputs: 重复的启动/停止请求、并发请求、档案锁。
- Outputs: 已在运行时启动返回现有运行时；启动中返回进行中；已停止时停止返回成功；并发启动不得产生两个内核。

### REQ-RUNTIME-004

- The system MUST verify the kernel file exists and its SHA-256 matches the installed manifest before first launch, and MUST refuse to start on mismatch.
- Inputs: 内核版本、平台、期望校验和、实际文件。
- Outputs: 校验通过或稳定错误 `KERNEL_CHECKSUM_MISMATCH`；不启动协议引擎、不启动进程。

### REQ-RUNTIME-005

- The system MUST check available memory, disk space, and the configured concurrency limit before spawning, and MUST reject or queue starts that exceed capacity.
- Inputs: 宿主或容器资源、运行中档案数、档案请求。
- Outputs: 允许启动、排队或返回 `QUOTA_EXCEEDED` / `INSUFFICIENT_RESOURCES`；不得先启动再因资源不足崩溃。

### REQ-RUNTIME-006

- The system MUST support headful launch on macOS by executing the kernel binary directly and using the host display.
- Inputs: macOS 宿主、有头模式、窗口尺寸。
- Outputs: 可见浏览器窗口；不使用 `open -a` 等丢失参数的方式；应用退出时受监督进程被停止或明确转为孤儿清理。

### REQ-RUNTIME-007

- The system MUST support Docker headful launch using one shared Xvfb display and openbox window manager, with the display resolution configured at container start.
- Inputs: `MIEN_SCREEN`（默认 `1920x1080x24`）、`DISPLAY=:99`、档案窗口尺寸。
- Outputs: 内核连接到 Xvfb；openbox 运行；窗口尺寸按档案设置；`screen.*` 为共享显示值并在 UI 中说明。

### REQ-RUNTIME-008

- The system MUST support an optional noVNC viewer that is disabled by default and protected by a short-lived one-time token or equivalent authentication.
- Inputs: 查看器开启请求、当前会话、容器网络配置。
- Outputs: 一次性查看 URL；令牌过期后失效；查看器不可用不得影响内核运行；不得默认把 VNC 端口暴露到公网。

### REQ-RUNTIME-009

- The system MUST treat headless mode as an explicit reduced-assurance mode and MUST mark it in API responses, profile details, and UI.
- Inputs: `headless` 偏好、启动请求。
- Outputs: 参数包含 `--headless=new`；响应包含 `reduced_assurance=true` 和限制说明；不得静默把有头请求降级为无头。

### REQ-RUNTIME-010

- The system MUST detect kernel exit, record exit code and recent logs, and apply bounded restart behavior without an infinite crash loop.
- Inputs: 内核退出事件、自动重启配置、退避窗口。
- Outputs: 状态 `crashed`；默认不自动重启；启用自动重启时最多 3 次，使用递增退避；稳定运行后重置计数。

### REQ-RUNTIME-011

- The system MUST stop a runtime by requesting graceful shutdown, waiting for a bounded timeout, then terminating the whole process group, and MUST stop the protocol engine and release the profile lock in all cases.
- Inputs: 停止请求、进程状态、超时配置。
- Outputs: 正常退出或强制终止后状态 `stopped`；协议引擎停止；锁释放；强制终止记录为审计事件。

### REQ-RUNTIME-012

- The system MUST reconcile non-terminal runtime records after a control-plane or container restart using PID, process start time, and command line, without killing processes it cannot safely attribute.
- Inputs: `runtimes` 记录、进程表、档案锁、数据目录。
- Outputs: 可归属进程被接管或安全终止；不可归属进程被标记为孤儿并按策略清理；不产生重复运行时。

### REQ-RUNTIME-013

- The system MUST expose CDP only on loopback by default, obtain the actual port from the kernel's `DevToolsActivePort` file, and MUST NOT publish it to the container network or LAN by default.
- Inputs: 自动化开关、用户数据目录、内核启动结果。
- Outputs: `ws://127.0.0.1:<port>/devtools/browser/<id>`；端口未就绪时启动失败；关闭自动化时不创建调试端口。

### REQ-RUNTIME-014

- The system MUST verify the effective Chromium sandbox state before serving traffic in Docker, and MUST refuse unsandboxed launch unless an explicit unsafe opt-in is set and audited.
- Inputs: 容器配置、内核启动参数、沙箱探测结果。
- Outputs: 沙箱生效时正常启动；无法确认时返回 `SANDBOX_UNAVAILABLE`；`ALLOW_UNSANDBOXED=1` 时允许并写入审计事件和启动警告。

### REQ-RUNTIME-015

- The system MUST write structured, rotated, redacted logs per runtime and MUST retain enough information to diagnose startup and crash failures.
- Inputs: 内核 stdout/stderr、运行时事件、错误码。
- Outputs: JSON Lines 日志；单文件上限 10 MB，最多 5 个文件；密码、Token、Cookie 和完整代理 URL 不出现。

### REQ-RUNTIME-016

- The system MUST provide liveness and readiness signals that distinguish process health from database, kernel, protocol engine, and display availability.
- Inputs: 服务端进程、SQLite、内核目录、协议引擎目录、Xvfb 状态。
- Outputs: `/healthz` 在进程存活时成功；`/readyz` 仅在数据库可用、内核与协议引擎校验通过且所需显示环境可用时成功。

## Verification

### VER-RUNTIME-001

- Method: 使用包含空格、引号和 Unicode 的路径启动内核，检查进程参数与环境。
- covers: `REQ-RUNTIME-001`
- Pass condition: 参数按数组传递，未调用 shell；数据目录唯一；进程组 ID 与 PID 有效。

### VER-RUNTIME-002

- Method: 驱动启动、成功、停止、崩溃和超时路径，收集事件流。
- covers: `REQ-RUNTIME-002`
- Pass condition: 事件序列符合状态机；非法转移被拒绝；最终状态与数据库一致。

### VER-RUNTIME-003

- Method: 并发发送 10 次启动请求和重复停止请求。
- covers: `REQ-RUNTIME-003`
- Pass condition: 只产生一个内核；重复启动返回同一运行时或进行中状态；重复停止成功。

### VER-RUNTIME-004

- Method: 篡改内核文件、删除内核、替换清单校验和。
- covers: `REQ-RUNTIME-004`
- Pass condition: 所有不匹配均拒绝启动，返回 `KERNEL_CHECKSUM_MISMATCH`，且无协议引擎或子进程残留。

### VER-RUNTIME-005

- Method: 在受限内存、受限磁盘和达到并发上限的环境中启动档案。
- covers: `REQ-RUNTIME-005`
- Pass condition: 超出容量时拒绝或排队，返回稳定错误；不出现启动后立即 OOM 或部分资源泄漏。

### VER-RUNTIME-006

- Method: 在 macOS Apple Silicon 上通过桌面应用启动有头档案。
- covers: `REQ-RUNTIME-006`
- Pass condition: 浏览器窗口出现；进程参数完整；退出应用后进程被停止或进入可清理状态。

### VER-RUNTIME-007

- Method: 在 Docker 中以默认和自定义 `MIEN_SCREEN` 启动有头档案，读取 `screen.*` 与窗口尺寸。
- covers: `REQ-RUNTIME-007`
- Pass condition: Xvfb 与 openbox 运行；屏幕值与配置一致；窗口尺寸按档案设置；UI 显示共享显示限制。

### VER-RUNTIME-008

- Method: 开启查看器，使用一次性 URL 访问，等待令牌过期后重试，并在查看器故障时检查浏览器状态。
- covers: `REQ-RUNTIME-008`
- Pass condition: 有效令牌可查看；过期令牌被拒绝；查看器故障不影响浏览器；默认关闭且未映射到公网。

### VER-RUNTIME-009

- Method: 分别以有头、无头和未确认无头请求启动档案，检查 argv 与响应标记。
- covers: `REQ-RUNTIME-009`
- Pass condition: 无头参数正确；响应和 UI 带降低保证；有头请求不会被静默改成无头。

### VER-RUNTIME-010

- Method: 让内核以不同退出码退出，启用和禁用自动重启，观察退避。
- covers: `REQ-RUNTIME-010`
- Pass condition: 状态变为 `crashed`；默认不重启；启用后最多 3 次递增退避；稳定运行后计数重置。

### VER-RUNTIME-011

- Method: 在页面加载、下载和大内存占用时停止档案，并在超时场景强制终止。
- covers: `REQ-RUNTIME-011`
- Pass condition: 优先正常退出；超时后进程组全部终止；协议引擎停止、锁释放；审计事件记录强制终止。

### VER-RUNTIME-012

- Method: 启动档案后强杀控制面并重启，检查进程归属与数据库状态。
- covers: `REQ-RUNTIME-012`
- Pass condition: 可归属进程被接管或安全停止；不可归属进程不被误杀；不产生第二个内核；状态最终一致。

### VER-RUNTIME-013

- Method: 开启自动化，读取 `DevToolsActivePort`，从容器外和局域网尝试连接。
- covers: `REQ-RUNTIME-013`
- Pass condition: 回环 CDP 可用；容器外和局域网无法直连；未开启自动化时无调试端口。

### VER-RUNTIME-014

- Method: 在启用命名空间沙箱、无法启用沙箱和显式无沙箱三种容器配置下启动。
- covers: `REQ-RUNTIME-014`
- Pass condition: 沙箱生效时成功；无法确认时拒绝；显式无沙箱时启动并记录审计与警告。

### VER-RUNTIME-015

- Method: 触发启动失败、崩溃和长时间运行，检查日志内容、轮转和脱敏。
- covers: `REQ-RUNTIME-015`
- Pass condition: 日志为 JSON Lines；轮转符合上限；敏感值不出现；诊断信息足够定位错误码。

### VER-RUNTIME-016

- Method: 分别停止数据库、移除内核、移除协议引擎、停止 Xvfb，调用健康与就绪端点。
- covers: `REQ-RUNTIME-016`
- Pass condition: `/healthz` 仅反映进程存活；`/readyz` 在依赖不可用时失败；恢复后重新成功。

## Related ADRs

- [ADR-0001：共享 Rust 核心与双宿主](../../adr/0001-shared-core-two-hosts.md)
- [ADR-0002：固定 fingerprint-chromium 148 并建立内核准入](../../adr/0002-pin-kernel-148.md)
- [ADR-0005：有头优先的显示策略](../../adr/0005-headful-first-display.md)
- [ADR-0007：协议引擎承载正向代理](../../adr/0007-protocol-engine-forward-proxy.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- [`../../contracts/kernel-manifest.schema.json`](../../contracts/kernel-manifest.schema.json)
