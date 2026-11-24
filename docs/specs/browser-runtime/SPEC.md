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
- `isolation`: 运行时隔离结论，取值 `enforced`、`unsandboxed` 或 `unknown`。`runtimes.isolation` 记录本次探测；`runtimes.last_known_isolation` 只在进入 `running` 的事务中更新。Profile/proof 只读后者，Runtime/事件只读本次结论。
- `reduced assurance`: 降级保证标记，由运行模式、直连配置与 `isolation` 共同决定；`headless`、`pseudo_headful`、`direct=true` 以及 `isolation != enforced` 的运行时保留该标记，已停止档案保留最后一次结论。
- `direct`: 档案未配置代理且显式进入直连模式的运行时标记；Profile 与 Runtime 必须返回 `direct=true`。该模式不创建代理入站/出站，不提供代理级远程 DNS 与失败关闭保证；仍应用内核支持的 DoH、IPv6 与非代理 UDP 控制，不能将内核开关等同于代理级保证。
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
- Transitions: `stopped -> starting`（start）、`starting -> running`（CDP 就绪，或“进程存活 + 代理模式下的入站就绪 + 有界宽限”；直连模式没有入站条件）、`starting -> stopping`（stop 取消、就绪前内核退出或 post-resource 失败）、`starting -> failed`（pre-resource 失败，确认没有代理资源或子进程）、`running -> stopping`（stop、内核退出、对账发现内核已退出/不能安全接管，或引擎重建失败）、`stopping -> stopped`（停止、取消或孤儿清理完成）、`stopping -> failed`（post-resource 启动失败或引擎重建失败的 teardown 完成）、`stopping -> crashed`（意外内核退出的 teardown 完成）、`crashed -> starting|stopped`、`failed -> starting|stopped`。`failed -> starting` 只允许用户显式重试且已确认无残留。引擎退出后的有界重建期间，可安全归属且仍存活的 `running` 内核保持原状态与代理绑定，但网络失败关闭；重建成功不产生虚假的 runtime 状态转移。需要 teardown 的路径不得直接写 `failed`/`crashed`；清理未完成保持 `stopping` 与 `last_error`。

### REQ-RUNTIME-003

- The system MUST make start and stop idempotent per profile and MUST allow at most one supervised runtime per profile.
- Inputs: 重复的启动/停止请求、并发请求、档案锁。
- Outputs: 已在运行时启动返回现有运行时；启动中返回进行中；`starting` 中的 stop 是取消，转入 `stopping`，清理完成后返回 `stopped`；`stopping` 中的启动请求被拒绝或返回在途停止状态，teardown 未完成、锁未释放时不得开始新启动；已停止时停止返回成功；对 `crashed`/`failed` 运行时执行停止只做幂等清理并返回 `stopped`；并发启动不得产生两个内核。

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
- Outputs: 内核退出后先进入 `stopping` 执行同一 teardown，清理完成再写 `crashed`；记录退出码与最近日志；默认不自动重启；启用自动重启时最多 3 次，使用递增退避；稳定运行后重置计数。内核退出发生在用户发起的停止流程中时，终态为 `stopped`，不记为 `crashed`，不发 `runtime.crashed`；由 teardown 自身终止进程组导致的内核退出，按发起 teardown 的原因决定终态（用户停止写 `stopped`，共享引擎故障或 post-resource 启动失败写 `failed`）；仅无在途停止请求的意外退出写 `crashed`。teardown 失败时保持 `stopping` 并重试，不得在资源仍存在时写 `crashed`。

### REQ-RUNTIME-011

- The system MUST stop a runtime by requesting graceful shutdown, waiting for a bounded timeout, then terminating the whole process group; it MUST remove the runtime's route rule and inbound and release its shared-outbound reference, while leaving the shared engine running for other profiles.
- Inputs: 停止请求、进程状态、超时配置。
- Outputs: 清理顺序固定为确认进程组全部退出 → 快照四个 `proxy_*` 绑定 → 按快照删除路由和入站 → 在同一事务中按快照 key 递减引用并清空四个绑定 → 释放端口与锁。引用归零时，保持共享引擎写锁，重新检查计数为零后删除 Xray 出站与数据库行；同 key 的获取引用与删除互斥，不在数据库事务内等待 Xray。强制终止写审计；key 已空时只重试孤儿清理，不重复递减。正常停止的终态为 `stopped`。
- Invariant: `proxy_outbounds.ref_count` 只统计非终态运行时；`stopped`、`crashed`、`failed` 运行时不得保留 `proxy_*` 字段或仍计数的出站引用。

### REQ-RUNTIME-012

- The system MUST reconcile non-terminal runtime records after a control-plane or container restart using PID, process start time, and command line, without killing processes it cannot safely attribute.
- Inputs: `runtimes` 记录、进程表、档案锁、数据目录。
- Outputs: 按内核进程归属分类，不能用引擎是否存活或代理资源是否完整判断内核无主。活且可安全接管的 `running` 内核保留状态、原 `proxy_port` 和引用供引擎修复/重建；直连内核无需引擎修复。`starting`/`stopping` 中断行，以及内核已退出或不能安全接管的 `running` 行，均经 `stopping` 执行 REQ-RUNTIME-011，完成后写 `stopped`。无法确认旧内核/进程组已退出时不盲目杀进程、不释放 `proxy_port`，保持 `stopping`、四个绑定字段及引用，撤除可归属路由/入站以失败关闭并记录隔离事件；TTL 到期也不得把该端口分配给其他档案。清理未完成保留 `last_error`，阻断该档案重启和服务就绪，后续对账或人工确认退出后才释放资源。

### REQ-RUNTIME-013

- The system MUST expose CDP only on loopback by default, obtain the actual port from the kernel's `DevToolsActivePort` file, and MUST NOT publish it to the container network or LAN by default.
- Inputs: 自动化开关、用户数据目录、内核启动结果。
- Outputs: 自动化开启时返回 `ws://127.0.0.1:<port>/devtools/browser/<id>`，端口未就绪时启动失败；关闭自动化时不创建调试端口，`starting → running` 改由“进程存活 + 代理模式下的入站就绪 + 有界启动宽限”判定；直连模式没有入站，只等待进程存活与宽限。

### REQ-RUNTIME-014

- The system MUST verify the effective Chromium sandbox state before creating any proxy resource or spawning the kernel, derive the runtime `isolation` verdict, and MUST refuse an unconfirmed sandbox on every host. Only the server may opt into an audited unsandboxed fallback after the containment probe and network boundary pass; desktop has no such opt-in.
- Inputs: 容器配置、内核清单 `sandbox_kind`、seccomp 配置、内核启动参数、沙箱探测结果、`ALLOW_UNSANDBOXED`。
- Outputs: 校验通过后在启动事务中写入 `runtimes.isolation`；任何宿主无法确认沙箱时返回 `SANDBOX_UNAVAILABLE`，且不创建出站、入站、路由规则或子进程。服务端只有显式 `ALLOW_UNSANDBOXED=1` 且通过包含性探针与独立 UID/网络边界时才继续启动，写 `isolation=unsandboxed` 并记录审计与警告；桌面端不提供无沙箱启动例外，无法确认 seatbelt/restricted-token 时同样返回 `SANDBOX_UNAVAILABLE`。`isolation=unknown` 只保留给没有成功运行时记录或历史迁移行，不能作为新启动的成功结论。

### REQ-RUNTIME-015

- The system MUST write structured, rotated, redacted logs per runtime and MUST retain enough information to diagnose startup and crash failures.
- Inputs: 内核 stdout/stderr、运行时事件、错误码。
- Outputs: JSON Lines 日志；单文件上限 10 MB，最多 5 个文件；密码、Token、Cookie 和完整代理 URL 不出现；`GET /api/v1/profiles/{id}/logs?limit=` 从运行时日志环或轮转文件读取有界、脱敏的尾部，`limit` 上限为 1000 行，超出上限的 `limit` 被拒绝，响应不含凭据、Cookie、授权头或完整 URL；桌面宿主提供等价 IPC/本地日志路径回退。

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
- Outputs: 探测后只写 `runtimes.isolation`；进入 `running` 时在同一事务将其复制到 `last_known_isolation`。Profile 的 `isolation` 始终来自 `last_known_isolation`，Runtime 响应与运行时事件使用本次 `isolation`；两类响应均含 `direct` 和 `reduced_assurance`，按各自输出字段推导：`reduced_assurance=true` 当且仅当 `mode` 为 `headless`/`pseudo_headful`、`direct=true` 或 `isolation != enforced`。失败尝试（包括探测通过但未进入 `running`）不得改写 `last_known_isolation`；没有成功运行记录时 Profile 为 `unknown`。成功启动的运行时只能是 `enforced` 或服务端显式授权的 `unsandboxed`。
- UI semantics: proof strip 的 IS 段只读 Profile 的 `isolation`（`last_known_isolation`），三种值与运行状态正交；`unknown` 用虚线轮廓与 `?`，`unsandboxed` 保留警告。只有 RT 段可用 `stopped` 的中性低透明度；从未启动的档案同时显示 RT=`stopped`、IS=`unknown`，停止不覆盖 IS、FP 或 PX 的警告。

### REQ-RUNTIME-019

- The system MUST preserve attributable live runtimes during bounded engine recovery, and MUST tear down affected runtimes before writing a terminal state if recovery fails; direct runtimes MUST remain unaffected.
- Inputs: 共享引擎故障或重建失败事件、运行时记录、进程与代理资源状态。
- Outputs: 引擎退出时仅依赖该引擎的运行时网络失败关闭；活且可安全归属的 `running` 内核保留代理字段与引用，按 REQ-PROXY-022 原端口重建成功后继续运行。管理 API 失联且不能证明数据面失败关闭，或有界重建失败时，先转入 `stopping`，终止进程组、删除入站/路由、按快照 key 在同一事务递减引用并清空 `proxy_*`、释放锁与配额，确认无残留后才写 `failed` 与终态事件。清理失败或归属不明按 REQ-RUNTIME-012 保留 `stopping` 与端口租约；已完成 teardown 的运行时只可由用户显式重试，rebuilt 事件不复活终态。

## Verification

### VER-RUNTIME-001

- Method: 使用包含空格、引号和 Unicode 的路径启动内核，检查进程参数与环境。
- covers: `REQ-RUNTIME-001`
- Pass condition: 参数按数组传递，未调用 shell；数据目录唯一；进程组 ID 与 PID 有效。

### VER-RUNTIME-002

- Method: 驱动启动、成功、停止、崩溃、超时、就绪前退出、共享引擎故障和控制面重启路径，收集事件流。
- covers: `REQ-RUNTIME-002`
- Pass condition: 事件序列符合状态机，包括 `starting -> stopping -> stopped`（取消）、`starting -> stopping -> crashed`（就绪前退出）、`starting -> failed`（pre-resource）、`starting -> stopping -> failed`（post-resource）、`running -> stopping -> crashed`、`running -> stopping -> failed`；非法转移被拒绝；teardown 失败时保持 `stopping`；终态无残留内核或代理资源；最终状态与数据库一致。

### VER-RUNTIME-003

- Method: 并发发送 10 次启动请求和重复停止请求。
- covers: `REQ-RUNTIME-003`
- Pass condition: 只产生一个内核；重复启动返回同一运行时或进行中状态；重复停止成功。

### VER-RUNTIME-004

- Method: 删除已安装内核文件、篡改解包后的可执行文件、只把发布资产摘要换成另一个值。
- covers: `REQ-RUNTIME-004`
- Pass condition: 只改发布资产摘要而 `binary_sha256` 仍匹配时启动成功；文件缺失返回 `KERNEL_NOT_FOUND`；可执行文件摘要与安装记录 `binary_sha256` 不符返回 `KERNEL_CHECKSUM_MISMATCH`，即使发布资产摘要匹配；两种失败都没有子进程、入站、路由或出站引用残留，且不留中间态。

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
- Pass condition: 内核退出后先进入 `stopping`，teardown 完成后状态为 `crashed`；退出码与日志可读；默认不重启；启用后最多 3 次递增退避；稳定运行后计数重置；teardown 失败时保持 `stopping`。

### VER-RUNTIME-011

- Method: 在页面加载、下载和大内存占用时停止档案，并在超时场景强制终止。
- covers: `REQ-RUNTIME-011`
- Pass condition: 优先正常退出；超时后进程组全部终止；入站、路由按 `ruleTag` 删除；非终态引用递减与 `proxy_*` 清空在同一事务完成，重试不会重复递减；`proxy_outbounds.ref_count` 不因终态行残留而漂移；共享引擎继续运行；审计事件记录强制终止。

### VER-RUNTIME-012

- Method: 在 `starting`、`stopping`、`running` 强杀控制面并重启；另模拟容器重启后内核全消失、活内核但引擎消失、资源缺项、PID 被复用和无法确认旧进程退出。
- covers: `REQ-RUNTIME-012`
- Pass condition: 活且可归属的 `running` 内核被接管并保留原端口进行引擎修复；内核已退出的 `running` 行与中断行都经 `stopping` 清理后写 `stopped`；不可归属进程不被误杀，未确认退出前保留端口、绑定、引用与 `stopping`，新档案不能取得该端口；确认退出后完成清理；无重复内核、幻影 `running` 或重复递减。

### VER-RUNTIME-013

- Method: 开启自动化并读取 `DevToolsActivePort`；另以关闭自动化的代理档案和直连档案启动，检查就绪条件。从容器外和局域网尝试连接。
- covers: `REQ-RUNTIME-013`
- Pass condition: 回环 CDP 可用；容器外和局域网无法直连；未开启自动化时无调试端口；代理档案等待入站就绪，直连档案不等待不存在的入站且仍能在宽限内进入 `running`。

### VER-RUNTIME-014

- Method: 在启用命名空间沙箱、无法启用沙箱和显式无沙箱（`ALLOW_UNSANDBOXED=1`）三种容器配置下启动，检查启动顺序、代理资源与 `isolation`。
- covers: `REQ-RUNTIME-014`
- Pass condition: 沙箱校验发生在创建代理资源和启动内核之前；生效时成功并写 `isolation=enforced`；无法确认时返回 `SANDBOX_UNAVAILABLE` 且未创建出站、入站、路由或内核进程；服务端显式无沙箱且通过包含性探针时启动、写 `isolation=unsandboxed` 并记录审计与警告；桌面端无法确认时拒绝启动。

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
- Pass condition: Runtime/事件使用本次 `isolation`，Profile/proof 使用 `last_known_isolation`；探测成功但 spawn/CDP 失败、写 `starting` 或 pre-resource 失败均不改上次成功结论；只有进入 `running` 的事务更新历史结论。重启后值仍保留；`reduced_assurance` 双向等价；从未启动的 RT=`stopped`、IS=`unknown` 同时可见；停止不覆盖 IS 警告。

### VER-RUNTIME-019

- Method: 在运行中注入共享引擎故障与重建失败，检查内核、代理资源与终态。
- covers: `REQ-RUNTIME-019`
- Pass condition: 引擎重建期间活且可归属的运行时保留状态、引用和端口，网络失败关闭；重建成功后原内核继续运行，失败才经 `stopping` 完成 teardown 后写 `failed`；清理未完成保留 `stopping`，不发终态；已终态不被 rebuilt 复活，直连全程不受影响。

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
