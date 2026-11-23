# 浏览器运行时与显示环境

> 本文件是运行时主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: 产品必须可靠地启动、监督和停止 fingerprint-chromium 进程，并在 macOS 宿主桌面与 Docker 无桌面环境中提供有头、无头和“伪有头”三种运行方式。
- In scope: 运行时状态机、启动/停止、进程组、崩溃处理、控制面重启对账、并发准入、内核校验、沙箱检查与隔离结论、CDP 暴露、日志、Xvfb/openbox/noVNC 生命周期。
- Out of scope: 指纹字段与 argv 语义（由 `fingerprint-engine` 负责）、代理协议与 DNS（由 `proxy-gateway` 负责）、档案元数据（由 `profile-management` 负责）。

## Terms and Interfaces

- `runtime`: 一个受监督的内核进程实例及其运行档案回环 HTTP 入站、路由规则、调试端口、日志和显示环境；共享 Xray 引擎由代理网关单独监督。
- `runtime state`: `stopped`、`starting`、`running`、`stopping`、`crashed`、`failed`。
- `process group`: 内核及其子进程组成的进程组，用于可靠终止和孤儿清理。
- `isolation`: 运行时隔离结论，取值 `enforced`、`unsandboxed` 或 `unknown`；持久化在 `runtimes.isolation`，并随档案与运行时响应返回，驱动四项 proof 的 isolation 段。
- `reduced assurance`: 降级保证标记，由运行模式与 `isolation` 共同决定；`headless`、`pseudo_headful` 以及 `isolation != enforced` 的运行时保留该标记，已停止档案保留最后一次结论。
- `headful`: 内核创建可见窗口；macOS 使用宿主桌面，Docker 使用 Xvfb。
- `headless`: 内核不创建可见窗口；标记为降低保证。
- `pseudo_headful`: 无头模式，依赖 fingerprint-chromium 的 `HeadlessChrome` → `Chrome` UA 补丁；其他 Headless 特征保留；标记为降低保证。`headless` 不承诺 UA 结果；在 pinned 148 上 UA 补丁可能是编译期默认，`pseudo_headful` 通过能力清单把该行为变成显式契约。
- Interface: `/api/v1/profiles/{id}/start`、`/stop`、`/runtime`、`/logs`；运行时事件 `runtime.starting`、`runtime.running`、`runtime.stopping`、`runtime.stopped`、`runtime.crashed`、`runtime.failed`；`runtimes` 表。

## Requirements

### REQ-RUNTIME-001

- The system MUST spawn the kernel as a direct child process using an argument array, a dedicated user-data directory, and a new process group; it MUST NOT invoke a shell.
- Inputs: 内核可执行路径、指纹 argv、档案数据目录、每运行档案回环 HTTP 入站、运行模式。
- Outputs: 内核 PID 与进程组 ID；子进程环境变量最小化；任何启动参数不得来自未校验的字符串拼接。

### REQ-RUNTIME-002

- The system MUST enforce the runtime state machine and MUST emit a state event for every transition.
- Inputs: 启动、停止、内核退出、超时、控制面重启。
- Outputs: 合法转移序列；非法转移必须被拒绝并记录；外部观察者可以通过事件流重建状态。
- Transitions: `stopped -> starting`（start）、`starting -> running`（CDP 就绪，或“进程存活 + 代理入站就绪 + 有界宽限”）、`starting -> crashed`（就绪前内核退出）、`starting -> failed`（校验、生成、沙箱、spawn 或超时失败）、`running -> stopping`（stop）、`running -> crashed`（内核退出）、`running -> failed`（共享引擎故障或重建失败，且在 teardown 完成后）、`stopping -> stopped`（正常退出或超时强制终止）、`crashed -> starting`（退避重试）、`crashed -> stopped` 与 `failed -> stopped`（清理或停止）。

### REQ-RUNTIME-003

- The system MUST make start and stop idempotent per profile and MUST allow at most one supervised runtime per profile.
- Inputs: 重复的启动/停止请求、并发请求、档案锁。
- Outputs: 已在运行时启动返回现有运行时；启动中返回进行中；已停止时停止返回成功；对 `crashed`/`failed` 运行时执行停止必须清理残留资源并转为 `stopped`；并发启动不得产生两个内核。

### REQ-RUNTIME-004

- The system MUST verify the selected kernel's installed executable before first launch, using the install record rather than the release manifest asset digest.
- Inputs: 档案 `kernel_version`、平台、`kernel_installs.binary_sha256`、实际可执行文件。
- Outputs: 校验通过并返回可执行路径；文件缺失返回 `KERNEL_NOT_FOUND`；可执行文件摘要与 `binary_sha256` 不符返回 `KERNEL_CHECKSUM_MISMATCH`；两种情况都 MUST 拒绝启动，且不得创建代理入站、路由规则、共享出站引用或子进程。
- Semantics: 清单 `sha256` 只是发布资产摘要，用于下载与安装校验；运行时只比对解包后的 `binary_sha256`。

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

- The system MUST stop a runtime by requesting graceful shutdown, waiting for a bounded timeout, then terminating the whole process group; it MUST remove the runtime's route rule and inbound and release its shared-outbound reference, while leaving the shared engine running for other profiles.
- Inputs: 停止请求、进程状态、超时配置。
- Outputs: 正常退出或强制终止后状态 `stopped`；清理顺序固定为终止进程组 → 按 `ruleTag` 删除路由规则 → 删除入站 → 清空 `proxy_outbound_key`、`proxy_inbound_tag`、`proxy_rule_tag`、`proxy_port` → 递减非终态共享出站引用 → 释放锁；强制终止记录为审计事件。
- Invariant: `proxy_outbounds.ref_count` 只统计非终态运行时；`stopped`、`crashed`、`failed` 运行时不得保留 `proxy_*` 字段或仍计数的出站引用。

### REQ-RUNTIME-012

- The system MUST reconcile non-terminal runtime records after a control-plane or container restart using PID, process start time, and command line, without killing processes it cannot safely attribute.
- Inputs: `runtimes` 记录、进程表、档案锁、数据目录。
- Outputs: 可归属进程被接管或安全终止；不可归属进程被标记为孤儿并按策略清理；不产生重复运行时；对 `starting`/`stopping` 中断行，终止可归属内核进程组、删除该档案的入站与规则、清空 `proxy_*`、递减非终态引用后写 `stopped`，只有 teardown 本身失败才写 `failed`；恢复完成后不保留中间态。

### REQ-RUNTIME-013

- The system MUST expose CDP only on loopback by default, obtain the actual port from the kernel's `DevToolsActivePort` file, and MUST NOT publish it to the container network or LAN by default.
- Inputs: 自动化开关、用户数据目录、内核启动结果。
- Outputs: 自动化开启时返回 `ws://127.0.0.1:<port>/devtools/browser/<id>`，端口未就绪时启动失败；关闭自动化时不创建调试端口，`starting → running` 改由“进程存活 + 代理入站就绪 + 有界启动宽限”判定。

### REQ-RUNTIME-014

- The system MUST verify the effective Chromium sandbox state before creating any proxy resource or spawning the kernel, derive the runtime `isolation` verdict, and MUST refuse an unconfirmed sandbox in Docker unless an explicit unsafe opt-in is set and audited.
- Inputs: 容器配置、内核清单 `sandbox_kind`、seccomp 配置、内核启动参数、沙箱探测结果、`ALLOW_UNSANDBOXED`。
- Outputs: 校验通过后在启动事务中写入 `runtimes.isolation`；Docker 中无法确认沙箱时返回 `SANDBOX_UNAVAILABLE`，且不创建出站、入站、路由规则或子进程；`ALLOW_UNSANDBOXED=1` 时继续启动，写 `isolation=unsandboxed` 并记录审计事件与启动警告；宿主桌面无法确认沙箱时保留 `isolation=unsandboxed` 或 `unknown`，不得阻止启动。

### REQ-RUNTIME-015

- The system MUST write structured, rotated, redacted logs per runtime and MUST retain enough information to diagnose startup and crash failures.
- Inputs: 内核 stdout/stderr、运行时事件、错误码。
- Outputs: JSON Lines 日志；单文件上限 10 MB，最多 5 个文件；密码、Token、Cookie 和完整代理 URL 不出现；`GET /api/v1/profiles/{id}/logs?limit=` 从运行时日志环或轮转文件读取有界、脱敏的尾部，超出上限的 `limit` 被拒绝，响应不含凭据、Cookie、授权头或完整 URL；桌面宿主提供等价 IPC/本地日志路径回退。

### REQ-RUNTIME-016

- The system MUST provide liveness and readiness signals that distinguish process health from database, kernel, shared protocol engine, and display availability.
- Inputs: 服务端进程、SQLite、内核目录、共享 Xray 引擎、Xvfb 状态。
- Outputs: `/healthz` 在进程存活时成功；`/readyz` 仅在数据库可用、内核与 Xray 引擎校验通过且所需显示环境可用时成功。

### REQ-RUNTIME-017

- The system MUST support `pseudo_headful` as an explicit reduced-assurance mode that requires the installed kernel manifest to declare `headless_chrome_ua_patch=true`, runs Chromium headless, and exposes only the UA-patch scope.
- Inputs: `pseudo_headful` 偏好、内核能力清单、启动请求。
- Outputs: 参数包含 `--headless=new`；响应包含 `mode=pseudo_headful`、`reduced_assurance=true`、`ua_scope=chrome` 和“其他 Headless 特征保留”的限制说明；能力缺失时拒绝启动，不得静默降级为 `headless` 或 `headful`。

### REQ-RUNTIME-018

- The system MUST persist and expose the isolation verdict as `isolation` with values `enforced`, `unsandboxed`, or `unknown`, and MUST derive the isolation proof and reduced-assurance markers from it.
- Inputs: 沙箱校验结果、运行模式、`runtimes` 记录、档案详情。
- Outputs: `runtimes.isolation` 持久化；运行时与档案响应包含 `isolation` 和 `reduced_assurance`；`reduced_assurance=true` 当且仅当 `mode` 为 `headless`/`pseudo_headful` 或 `isolation != enforced`；已停止档案仍返回最后一次 `isolation`，并显示 `unsandboxed` 或 `unknown` 提示。
- UI semantics: proof strip 的 isolation 段由该字段驱动；`unknown` 使用虚线轮廓与 `?` 文本说明，`stopped` 使用中性低透明度，两者不得呈现为相同状态。

### REQ-RUNTIME-019

- The system MUST tear down a runtime before writing any terminal state when the shared proxy engine fails or must be rebuilt.
- Inputs: 共享引擎故障或重建失败事件、运行时记录、进程与代理资源状态。
- Outputs: 依次终止内核进程组、删除该档案的入站与路由、清空 `proxy_*`、递减非终态出站引用、释放档案锁与配额，然后写 `failed` 并发出 `runtime.failed` 事件；`crashed`、`failed`、`stopped` 之后不得存在仍在运行的内核或仍计数的代理资源；引擎恢复后由用户显式重试启动，不自动把运行时改回 `running`。

## Verification

### VER-RUNTIME-001

- Method: 使用包含空格、引号和 Unicode 的路径启动内核，检查进程参数与环境。
- covers: `REQ-RUNTIME-001`
- Pass condition: 参数按数组传递，未调用 shell；数据目录唯一；进程组 ID 与 PID 有效。

### VER-RUNTIME-002

- Method: 驱动启动、成功、停止、崩溃、超时、就绪前退出、共享引擎故障和控制面重启路径，收集事件流。
- covers: `REQ-RUNTIME-002`
- Pass condition: 事件序列符合状态机，包括 `starting -> crashed` 与 `running -> failed`；非法转移被拒绝；终态无残留内核或代理资源；最终状态与数据库一致。

### VER-RUNTIME-003

- Method: 并发发送 10 次启动请求和重复停止请求。
- covers: `REQ-RUNTIME-003`
- Pass condition: 只产生一个内核；重复启动返回同一运行时或进行中状态；重复停止成功。

### VER-RUNTIME-004

- Method: 删除已安装内核文件、篡改解包后的可执行文件、只把发布资产摘要换成另一个值。
- covers: `REQ-RUNTIME-004`
- Pass condition: 文件缺失返回 `KERNEL_NOT_FOUND`；可执行文件摘要与安装记录 `binary_sha256` 不符返回 `KERNEL_CHECKSUM_MISMATCH`，即使发布资产摘要匹配；两种失败都没有子进程、入站、路由或出站引用残留，且不留中间态。

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
- Pass condition: 优先正常退出；超时后进程组全部终止；入站、路由按 `ruleTag` 删除，`proxy_*` 字段清空后非终态引用递减；`proxy_outbounds.ref_count` 不因终态行残留而漂移；共享引擎继续运行；审计事件记录强制终止。

### VER-RUNTIME-012

- Method: 在 `starting` 与 `stopping` 中间态强杀控制面并重启，检查进程归属、代理资源与数据库状态。
- covers: `REQ-RUNTIME-012`
- Pass condition: 可归属进程被接管或安全停止；不可归属进程不被误杀；中断的 `starting`/`stopping` 行在 teardown 后写 `stopped`，仅 teardown 失败写 `failed`；不产生第二个内核，不留中间态或未计数的出站引用。

### VER-RUNTIME-013

- Method: 开启自动化，读取 `DevToolsActivePort`，从容器外和局域网尝试连接。
- covers: `REQ-RUNTIME-013`
- Pass condition: 回环 CDP 可用；容器外和局域网无法直连；未开启自动化时无调试端口。

### VER-RUNTIME-014

- Method: 在启用命名空间沙箱、无法启用沙箱和显式无沙箱（`ALLOW_UNSANDBOXED=1`）三种容器配置下启动，检查启动顺序、代理资源与 `isolation`。
- covers: `REQ-RUNTIME-014`
- Pass condition: 沙箱校验发生在创建代理资源和启动内核之前；生效时成功并写 `isolation=enforced`；无法确认时返回 `SANDBOX_UNAVAILABLE` 且未创建出站、入站、路由或内核进程；显式无沙箱时启动、写 `isolation=unsandboxed` 并记录审计与警告。

### VER-RUNTIME-015

- Method: 触发启动失败、崩溃和长时间运行，检查日志内容、轮转和脱敏，并通过 `GET /api/v1/profiles/{id}/logs?limit=` 读取尾部，包括超出上限的 `limit`。
- covers: `REQ-RUNTIME-015`
- Pass condition: 日志为 JSON Lines；轮转符合单文件与文件数上限；敏感值、Cookie、授权头与完整 URL 不出现；日志尾部有界且按时间顺序；超限 `limit` 被拒绝；桌面回退提供等价尾部。

### VER-RUNTIME-016

- Method: 分别停止数据库、移除内核、移除协议引擎、停止 Xvfb，调用健康与就绪端点。
- covers: `REQ-RUNTIME-016`
- Pass condition: `/healthz` 仅反映进程存活；`/readyz` 在依赖不可用时失败；恢复后重新成功。

### VER-RUNTIME-017

- Method: 在内核能力清单声明和未声明 `headless_chrome_ua_patch` 的两种情况下启动 `pseudo_headful`，检查 argv、UA、响应标记和其他 Headless 特征。
- covers: `REQ-RUNTIME-017`
- Pass condition: 声明时启动成功并返回 `ua_scope=chrome`；未声明时拒绝并返回稳定错误；不静默降级；UI/API 显示降低保证和其他 Headless 特征仍保留。

### VER-RUNTIME-018

- Method: 分别产生 `enforced`、`unsandboxed` 与 `unknown` 的运行时，并检查已停止档案的档案详情与 proof strip。
- covers: `REQ-RUNTIME-018`
- Pass condition: `runtimes.isolation` 持久化并在重启后仍以数据库为准；运行时与档案响应包含 `isolation` 及按模式推导的 `reduced_assurance`；已停止档案仍显示最后一次隔离结论与降低保证提示；`unknown` 的虚线与 `?` 文本和 `stopped` 的中性低透明度可区分。

### VER-RUNTIME-019

- Method: 在运行中注入共享引擎故障与重建失败，检查内核、代理资源与终态。
- covers: `REQ-RUNTIME-019`
- Pass condition: 每个受影响运行时都先完成 teardown 再写 `failed`；内核进程组、入站、路由、出站引用、锁与配额均被释放；`runtime.failed` 事件可观察；引擎恢复后需显式重试且不自动恢复 `running`。

## Related ADRs

- [ADR-0001：共享 Rust 核心与双宿主](../../adr/0001-shared-core-two-hosts.md)
- [ADR-0002：固定 fingerprint-chromium 148 并建立内核准入](../../adr/0002-pin-kernel-148.md)
- [ADR-0005：有头优先的显示策略](../../adr/0005-headful-first-display.md)
- [ADR-0009：单实例 Xray 动态协议引擎与连接复用](../../adr/0009-shared-xray-engine.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- [`../../contracts/kernel-manifest.schema.json`](../../contracts/kernel-manifest.schema.json)
