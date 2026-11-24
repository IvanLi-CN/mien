# Mien 详细设计

## 1. 目的与边界

本文把 [`ARCHITECTURE.md`](ARCHITECTURE.md) 的系统级设计落到可实现的模块、接口、并发、错误、配置和存储约束。技术选型与替代方案见 [`TECH-STACK.md`](TECH-STACK.md)；长期能力契约见 [`specs/`](specs/)。本文不重复产品需求、ADRs 的完整取舍或测试矩阵。

## 2. 模块与依赖方向

```text
mien-desktop ─┐
mien-server  ─┼─> mien-core
mien-cli     ─┘
```

- `mien-core` 不依赖 Tauri、axum、React 或具体数据库驱动；领域层只依赖标准库、`serde`、`time` 和轻量工具 crate。
- `mien-core` 内部按领域模块组织：`profile`、`fingerprint`、`proxy`、`runtime`、`kernel`、`engine`、`storage`（仓储 trait 与用例）、`security`、`events`。
- `mien-store-sqlite` 是唯一依赖 `sqlx` 的 crate，实现 `mien-core` 的仓储端口并提供迁移；宿主在启动时装配它，核心不知道 SQLite 存在。
- 端口（trait）定义在核心内，适配器在 `mien-store-sqlite` 或宿主 crate 中实现。
- 宿主只做 DTO 转换、认证、静态资源、窗口和平台集成；业务规则不得在宿主复制。

依赖规则：

- 领域层不得直接访问文件系统、进程、网络、钥匙串或 SQLite；需要这些能力时只调用 trait。
- 仓储 trait（档案、运行时、代理、审计、幂等、事件、Token）定义在 `mien-core`；SQL 查询、连接池和迁移不得出现在领域模块或宿主 crate。
- 适配器不得反向依赖宿主框架。
- 所有跨层数据使用显式 DTO 或领域类型，禁止把 axum/Tauri 类型传入核心。
- 时间、随机数、进程启动、密钥读取和事件发布必须通过 trait 注入，保证测试可替代。

## 3. 领域类型与端口

### 3.1 标识与状态

- `ProfileId(Ulid)`、`RuntimeId(Ulid)`、`KernelVersion`、`EngineVersion`。
- `SpecSchema`、`KernelManifest`、`EngineManifest` 各自独立版本。
- `RuntimeState`：`Stopped`、`Starting`、`Running`、`Stopping`、`Crashed`、`Failed`。
- 合法转移：`Stopped → Starting`、`Starting → Running|Stopping|Failed`、`Running → Stopping`、`Stopping → Stopped|Failed|Crashed`、`Crashed → Starting|Stopped`、`Failed → Starting|Stopped`。活代理内核遇引擎故障可保持 `Running` 并网络失败关闭以等待有界重建；仅恢复耗尽或无法证明数据面失败关闭时进入 `Stopping` teardown。直连运行时不受引擎事件影响。teardown 未完成保留 Stopping/last_error；任何终态不得有活内核或计数资源。
- `ProfileStatus`：`Active`、`Archived`、`Deleted`，对外序列化为 `active`/`archived`/`deleted`；`Deleted` 必须携带 `deleted_at`，永久删除失败时携带 `deletion_error` 并保持 `Deleted`；软删除目录移动失败时原状态可携带 `deletion_error` 供重试；`Deleted` 不能启动。
- `Isolation`：`Enforced`、`Unsandboxed`、`Unknown`；每次探测写本次 `runtimes.isolation`，只有进入 running 的同一事务才复制到 `last_known_isolation`。Profile/proof 只读后者；Runtime/event 只读本次值。成功启动只能是 enforced 或服务端显式授权的 unsandboxed；未知只用于无成功记录或历史行。
- `SandboxKind`：`Namespace`、`Seatbelt`、`Win32RestrictedToken`、`None`，来自内核能力清单 `sandbox_kind`；`None` 表示构件没有内核级沙箱，桌面端必须返回 `SANDBOX_UNAVAILABLE`，服务端只有显式 `ALLOW_UNSANDBOXED=1` 且通过包含性探针与独立 UID/网络边界时才按 `isolation=unsandboxed` 启动并审计；Windows 普通渲染进程不使用 AppContainer。
- `RunMode`：`Headful`、`Headless`、`PseudoHeadful`；对外序列化为 `headful`、`headless`、`pseudo_headful`。
- `ProxyConfig`：`Http`、`Https`、`Socks5`、`Vless`、`Vmess`、`Shadowsocks`、`Trojan`、`Hysteria2`、`Wireguard` 的 tagged enum；秘密字段使用 `SecretString`，`Debug` 输出必须脱敏。TUIC 不进入 v1。
- `LaunchPlan`：内核 argv、环境变量、用户数据目录、代理入口、运行模式、是否启用 CDP、日志目标。
- `RuntimeHandle`：运行时标识、档案标识、运行模式、`isolation`、内核 PID、代理端口、CDP 端口、`proxy_inbound_tag`、`proxy_outbound_key`、`proxy_rule_tag`、取消令牌。
- `ProxyEngineHandle`：共享 Xray 进程 PID、管理 API 端口、启动时间、状态和取消令牌。

### 3.2 核心端口

```rust
pub trait SecretStore {
    fn get(&self, key: &SecretKey) -> Result<SecretString>;
    fn put(&self, key: &SecretKey, value: &SecretString) -> Result<()>;
    fn delete(&self, key: &SecretKey) -> Result<()>;
}

pub trait ProcessLauncher {
    fn spawn(&self, plan: &LaunchPlan) -> Result<ProcessHandle>;
    fn signal(&self, handle: &ProcessHandle, signal: Signal) -> Result<()>;
    fn wait(&self, handle: &ProcessHandle, timeout: Duration) -> Result<ExitStatus>;
    fn kill_group(&self, handle: &ProcessHandle) -> Result<()>;
}

pub trait DisplayProvider {
    fn ensure(&self, mode: DisplayMode) -> Result<DisplayHandle>;
    fn viewer_url(&self, runtime: &RuntimeId) -> Result<Option<ViewerUrl>>;
}

pub trait Clock {
    fn now(&self) -> OffsetDateTime;
}

pub trait EventSink {
    fn publish(&self, event: DomainEvent) -> Result<EventId>;
}
```

`ProcessLauncher` 必须接收参数数组和显式环境变量，不接收 shell 字符串。`DisplayProvider` 在 macOS 返回宿主显示，在 Docker 返回共享 Xvfb。`EventSink` 先持久化事件再广播，保证 WebSocket 断线可按标识回放。

### 3.3 注册表与校验

- `KernelRegistry`：解析版本与平台、安装时校验 `asset_sha256`、启动前校验可执行文件 `binary_sha256`、读取内核能力清单。
- `EngineRegistry`：解析 Xray-core 版本与平台、安装时校验 `asset_sha256`、启动前校验 `binary_sha256`、读取协议能力清单与许可信息。
- `FingerprintValidator`：schema、一致性、能力清单校验，输出规范化规格。
- `ProxyValidator`：协议、传输、TLS/REALITY、凭据字段、能力清单校验，输出可生成的引擎配置。
- `BuildArgv`：纯函数，输入规格与运行时参数，输出确定性参数数组。
- `BuildEngineOutbound`：纯函数，输入代理配置与凭据引用，输出 Xray 出站配置和稳定 `outbound_key`；相同 key 必须复用出站。
- `BuildInbound`：纯函数，输入运行时标识与 `proxy_port`，输出回环 HTTP 入站配置和标签。
- `BuildRouteRule`：纯函数，输入入站标签、出站标签与稳定 `ruleTag`，输出带该 tag 的 Xray 路由规则。
- `EngineController`：封装 Xray gRPC 管理 API，提供 ensure、add/reuse outbound、add inbound、add rule（`shouldAppend=true`）、按 `ruleTag` remove、reconcile 和引用计数操作；启动时断言 blackhole 是第一个出站。

## 4. 启动与停止事务

### 4.1 启动顺序

1. 宿主完成认证与授权，调用核心启动用例。
2. 读取档案；拒绝已归档、已删除或正在删除的档案。
3. 获取档案级 actor 锁；已有启动/停止操作时按幂等语义返回。
4. 获取全局运行时配额；检查可用内存、磁盘和并发上限。
5. 校验指纹规格（含窗口尺寸，唯一来源）、代理配置、内核与协议引擎能力清单；`pseudo_headful` 还要求 `headless_chrome_ua_patch=true`。
6. 在事务内写入 `runtimes(state=starting)` 并发布 `runtime.starting`，保留 `last_known_isolation` 中上一次成功结论；此后任何失败都能被对账发现。
7. 校验内核与引擎可执行文件的 `binary_sha256`（不是清单资产摘要）；失败则不创建任何临时资源，按 `starting -> failed` 写回。
8. 沙箱验证：按内核清单 `sandbox_kind` 探测实际生效状态，得出 `isolation=enforced`；任何宿主无法确认且未启用服务端 `ALLOW_UNSANDBOXED=1` 时返回 `SANDBOX_UNAVAILABLE`，不创建代理资源、不启动进程。服务端显式无沙箱还必须通过包含性探针与独立 UID/网络边界，之后写 `isolation=unsandboxed` 并审计；桌面无法确认时直接拒绝，不存在无沙箱例外。探测通过后只持久化本次 `isolation`；所有尚未进入 `running` 的路径都不得覆盖 `last_known_isolation`。
9. 代理模式：分配回环代理端口；确保共享 Xray 引擎运行并连接 gRPC 管理 API；断言 blackhole 是第一个出站；计算 `outbound_key`，添加或复用出站并增加引用计数；添加带稳定 `ruleTag` 的入站和路由（`shouldAppend=true`）。直接模式跳过本步骤。
10. 生成内核 argv；代理模式注入 `--proxy-server=http://127.0.0.1:<proxy_port>` 并应用 DoH/IPv6/非代理 UDP 防护参数；直接模式不注入代理参数，也不声称代理级远程 DNS/失败关闭保证，但仍应用内核支持的 `webrtc`/`network` 标志并记录降低保证；子进程 `env_clear` 后只注入白名单环境。
11. 启动内核；自动化开启时等待 `DevToolsActivePort`，关闭时等待进程存活、代理模式下入站就绪（直连模式跳过入站条件）和有界启动宽限。
12. 在同一事务中持久化 `runtimes=running`，并把本次 `isolation` 复制到 `last_known_isolation`；发布 `runtime.running`，返回运行时句柄。只有此步更新 Profile/proof 的历史结论。

失败回滚顺序与启动相反：终止内核进程组、按 `ruleTag` 删除路由规则与入站、在同一事务中递减非终态出站引用并清空 `proxy_*`、释放端口、释放配额、释放 actor 锁，然后写入 `failed` 与稳定错误码。teardown 失败时保留 `stopping` 与 `last_error` 并重试，不得提前写终态。pre-resource 失败只需释放配额与锁，并可直接写 `failed`。共享引擎本身不因单个运行时失败而退出。回滚中的二次失败必须记录，但不得掩盖原始错误；teardown 必须幂等且只执行一次。

### 4.2 停止顺序

1. 状态改为 `stopping`，发布事件。
2. 向内核发送正常退出信号，等待数据落盘。
3. 超时后终止内核进程组。
4. 快照该运行时的 `proxy_outbound_key`、`proxy_inbound_tag`、`proxy_rule_tag` 和 `proxy_port`，再按 `ruleTag` 删除路由规则与入站。
5. 确认内核进程组退出后，在同一事务中按快照 key 递减引用并清空四个 `proxy_*` 字段；key 已空时不重复递减。共享引擎写锁覆盖计数复核、Xray 出站删除和零引用行删除；同 key 的启动获取引用必须持有同一锁，不得在数据库事务内等待 Xray。
6. Xray 删除失败则保留 `ref_count=0` 孤儿供对账；无法确认旧进程退出则不执行步骤 5，保留 `stopping`、绑定、引用和端口租约，撤除可归属路由/入站后记录隔离事件，不得因 TTL 到期而复用端口。
7. 释放端口、配额与 actor 锁；共享引擎保持运行。
8. 写入 `stopped`，发布事件；强制终止写入审计事件。

停止对已停止档案幂等返回成功。停止失败时保留 `runtimes` 记录和诊断信息，允许重试清理；重试仍从步骤 4 开始，不重复终止已退出进程。

### 4.3 重启对账

控制面启动时读取非终态 `runtimes`：

- 通过 PID、进程启动时间和 API 端口确认共享 Xray 引擎；通过 PID、命令行和数据目录确认各内核。
- 对活且可接管的 `running` 内核保留原端口、绑定与引用，不能因引擎退出或资源缺项就把它判为无主；引擎可接管时补齐资源，不可接管时按 REQ-PROXY-022 有界重建这些运行时的配置（原端口/tag、`shouldAppend=true`）；直连档案不参与。
- 对 `starting`/`stopping` 中断行及内核已退出或无法安全接管的 `running` 行，先转 `stopping`，执行第 4.2 节快照后递减的 teardown，完成才写 `stopped`；无法确认进程退出则保留端口与非终态，不能盲目终止或回收。
- 共享引擎重建失败时，对每个受影响运行时先转入 `stopping`，执行第 4.1 节的同一 teardown，完成后才写 `failed`；teardown 失败保留 `stopping` 与 `last_error` 并重试；不允许终态背后存在活内核或仍被计数的出站。
- 清理无对应进行中检查的 `chk:<check_id>:*` 临时入站、规则和出站；对 `rt:<id>:*` 按运行时归属对账，不误删有效资源。
- 只有确认无活进程且 teardown 完成后才能删除锁和端口引用并发终态事件；不确定归属或清理失败时保留 `stopping` 与诊断，服务就绪失败，重试或人工确认后再完成清理。

## 5. 并发与锁

- 每个档案一个 actor，命令通过 `mpsc` 串行化；启动/停止不会并发执行。
- 全局 `Semaphore` 控制最大运行档案数；资源检查在获取配额后进行，失败立即释放。
- SQLite 使用连接池，事务短小；禁止在持有数据库事务时等待进程、网络或引擎就绪。
- 锁顺序固定为：档案 actor → 全局配额 → 共享引擎写锁 → 代理端口/入站 → 内核；反向释放。共享引擎写锁覆盖完整的出站获取/释放、归零复核、Xray 删除与数据库行删除序列，而非仅串行化单次 API；数据库事务保持短小，不跨网络等待。
- 事件持久化与广播通过独立任务解耦；广播无订阅者不影响事件落库。
- 进程等待使用异步子进程 API；文件哈希、目录清理等可能阻塞的操作放入阻塞线程池。
- 关闭时设置全局取消令牌，按启动逆序停止运行时，最后执行 WAL checkpoint。

## 6. 错误模型

核心返回 `DomainError { code, message, retryable, details }`。宿主映射为 HTTP 或 Tauri 错误；`details` 只能包含非敏感字段。

| 错误族 | 示例 | HTTP | 说明 |
| --- | --- | --- | --- |
| 认证与授权 | `AUTH_REQUIRED`、`AUTH_INVALID`、`AUTH_RATE_LIMITED`、`AUTH_PASSWORD_CHANGE_REQUIRED`、`AUTH_ORIGIN_INVALID`、`EVENT_TICKET_INVALID`、`FORBIDDEN_SCOPE` | 401/403/429 | 缺失、过期或无效凭据；`Origin` 不被信任；已认证但无权限或必须先改密 |
| 资源与生命周期 | `PROFILE_NOT_FOUND`、`PROXY_NOT_FOUND`、`RUNTIME_NOT_FOUND`、`TOKEN_NOT_FOUND`、`GROUP_NOT_FOUND`、`TAG_NOT_FOUND`、`VIEWER_NOT_AVAILABLE`、`PROFILE_ARCHIVED`、`PROFILE_DELETED`、`PROFILE_RESTORE_UNAVAILABLE`、`PROFILE_PURGE_FAILED`、`PROFILE_IMPORT_CLEANUP_FAILED`、`RUNTIME_CONFLICT`、`RESOURCE_IN_USE`、`IDEMPOTENCY_CONFLICT`、`PROFILE_IMPORT_CONFLICT`、`GROUP_NAME_CONFLICT`、`TAG_NAME_CONFLICT` | 404/409/500 | 资源不存在为 404；状态、归属、幂等键或命名冲突为 409；软删除移动失败保持原状态，purge 失败保持 deleted；导入回滚后的目录清理失败用独立可重试 500，不重建档案行。 |
| 输入与能力 | `VALIDATION_FAILED`、`FINGERPRINT_INVALID`、`PROXY_CONFIG_INVALID`、`SHARE_LINK_INVALID`、`RUNTIME_MODE_UNSUPPORTED`、`KERNEL_UNSUPPORTED`、`UNSUPPORTED_PROTOCOL`、`TUIC_NOT_SUPPORTED` | 422 | `*_INVALID`、`*_UNSUPPORTED`、`*_NOT_SUPPORTED` 表达输入或能力不符 |
| 配额与资源不足 | `QUOTA_EXCEEDED`、`INSUFFICIENT_RESOURCES` | 429/503 | 并发上限可重试；内存、磁盘、描述符不足为 503 |
| 内核 | `KERNEL_NOT_FOUND`、`KERNEL_CHECKSUM_MISMATCH`、`KERNEL_VERSION_NOT_FOUND`、`KERNEL_UNSUPPORTED` | 500/404/422 | 已安装文件缺失或 `binary_sha256` 不符；激活不存在的安装版本为 404；不兼容组合在启动前拒绝 |
| 内核宿主环境 | `SANDBOX_UNAVAILABLE`、`DISPLAY_UNAVAILABLE` | 500/503 | 所有宿主在沙箱无法确认时拒绝启动；服务端无沙箱仅在显式授权与包含性探针后允许；显示环境不可用可重试 |
| 代理引擎 | `PROXY_ENGINE_NOT_FOUND`、`PROXY_ENGINE_CHECKSUM_MISMATCH`、`PROXY_ENGINE_VERSION_NOT_FOUND`、`PROXY_ENGINE_START_FAILED`、`PROXY_ENGINE_NOT_READY`、`PROXY_ENGINE_API_UNAVAILABLE`、`PROXY_ENGINE_CRASHED`、`PROXY_INBOUND_CONFLICT`、`PROXY_OUTBOUND_REFCOUNT_MISMATCH` | 500/404/503/409 | 引擎安装、启动、API、端口与引用计数失败；激活不存在的安装版本为 404 |
| 代理节点 | `PROXY_CONNECTION_REFUSED`、`PROXY_AUTH_FAILED`、`PROXY_TLS_FAILED`、`PROXY_TIMEOUT`、`PROXY_DNS_FAILED` | 200 | 节点侧失败一律出现在代理检查的 200 `ProxyCheck.error_code` 与事件中，不用 HTTP 502；引擎未就绪由 `PROXY_ENGINE_NOT_READY` 控制面 503 表达 |
| 运行时 | `RUNTIME_SPAWN_FAILED`、`RUNTIME_CDP_TIMEOUT`、`RUNTIME_MODE_UNSUPPORTED`、`DIRECT_EGRESS_DISABLED` | 500/422/403 | 进程创建或就绪失败；模式不支持在启动前拒绝；服务端未授权直连出口时拒绝启动 |
| 存储 | `STORAGE_BUSY`、`STORAGE_MIGRATION_FAILED`、`STORAGE_INTEGRITY_FAILED` | 503/500 | 忙碌可重试；迁移与完整性失败需人工处理 |
| 内部 | `INTERNAL_UNEXPECTED` | 500 | 未分类错误，必须带 request_id |

前缀表示错误所属的名词族；HTTP 状态以 [`ERROR-CODES.md`](ERROR-CODES.md) 的逐码表为准，不能仅由后缀推导（例如 `AUTH_INVALID` 为 401、`KERNEL_NOT_FOUND` 为 500、`PROFILE_DELETED` 为 409）。实现以目录与 OpenAPI 枚举为准；CI 校验目录、枚举和代码三方一致。代理检查的节点错误不做 HTTP 502：`POST /api/v1/proxies/{id}/check` 返回 200，失败写在 `ProxyCheck.error_code` 并同时发事件。

错误消息不得包含代理凭据、UUID、私钥、完整分享链接、Cookie、Token 或完整启动参数。可重试错误必须返回 `retryable=true` 和重试建议。

## 7. 配置矩阵

配置优先级从高到低：CLI 参数、环境变量、数据库设置、平台默认值。秘密只允许来自钥匙串、环境变量或 0600 文件，不得写入数据库设置。

| 变量 | 默认 | 作用 |
| --- | --- | --- |
| `MIEN_DATA_DIR` | 平台数据目录或 `/data` | 数据库、档案、内核、引擎和日志根 |
| `MIEN_BIND` | 裸机服务端 `127.0.0.1:8080`；Docker 镜像 `0.0.0.0:8080`；桌面未设置 | 服务端控制面监听地址；桌面不监听；容器内监听回环无法被 `docker run -p` 到达，因此 Docker 默认非回环并强制 `MIEN_PUBLIC_URL`；裸机对外发布时显式设为非回环并同时设置 `MIEN_PUBLIC_URL` |
| `MIEN_DESKTOP_AUTOMATION_BIND` | 未设置 | 桌面可选自动化地址；显式设置后才监听，且必须是 `127.0.0.1`/`::1`，非回环值启动失败 |
| `MIEN_MASTER_KEY` | 无 | 服务端主密钥；未提供时使用 `/data/secrets/master.key` |
| `MIEN_ADMIN_PASSWORD` | 无 | 首次启动管理员密码引导值；与 `MIEN_ADMIN_PASSWORD_FILE` 二选一；两者都未提供时生成密码写入 `<data-root>/secrets/admin-bootstrap` |
| `MIEN_ADMIN_PASSWORD_FILE` | 无 | 以 0600 文件提供管理员密码；首次改密后删除应用生成的文件并清除引导标记；外部挂载 Secret 由部署者撤销 |
| `MIEN_PUBLIC_URL` | 无；非回环服务端必填 | 服务端对外 https 基址；用于生成绝对链接、校验 `Origin` 和设置 `Secure` Cookie。`MIEN_BIND` 非回环且未设置时启动失败 |
| `MIEN_TRUSTED_PROXIES` | 空 | 可信反向代理 CIDR/地址列表；仅这些来源的 `X-Forwarded-*` 被采信 |
| `MIEN_SCREEN` | `1920x1080x24` | Docker Xvfb 分辨率 |
| `MIEN_MAX_RUNTIMES` | `10` | 最大并发运行档案数 |
| `MIEN_LOG` | `info` | 日志级别与格式 |
| `ALLOW_UNSANDBOXED` | `false` | 仅服务端生效。显式允许无沙箱内核；必须通过包含性探针与独立 UID/网络边界并审计，启动写入 `isolation=unsandboxed`；桌面端无此例外 |
| `MIEN_ALLOW_DIRECT_EGRESS` | `false` | 服务端是否允许直连档案；未设置时返回 `DIRECT_EGRESS_DISABLED`，设置后必须写审计并调整网络策略 |
| `MIEN_VIEWER` | `false` | 是否启用 noVNC 查看器 |
| `MIEN_UPDATE_CHECK` | `false` | 是否检查更新；默认不联网 |

macOS 使用 `~/Library/Application Support/Mien` 与 `~/Library/Logs/Mien`；Docker 使用 `/data` 与标准输出日志。

## 8. 存储与迁移

- SQLite 开启 WAL、外键、`busy_timeout=5000`；写入使用短事务。
- 迁移正向执行、版本化、不可变；不提供自动降级。迁移前创建数据库备份，失败时保持原库可打开。
- 迁移文件只增不改；开发期变更通过新迁移完成。本版本需要：回填后删除 `profiles.window_json`（回填步骤见 DATABASE-SCHEMA §5），新增 `profiles.deletion_error`、`runtimes.isolation`、`runtimes.last_known_isolation`、`admin_credentials`、安装表的 `asset_sha256`/`binary_sha256` 与 `idx_profile_tags_tag`。
- 敏感字段以 AES-256-GCM 加密；密文包含版本、nonce 和 AAD（记录标识 + 字段名）。
- `event_log` 按 REQ-API-011 的 7 天且 payload 最多 100 MiB 裁剪最旧 sequence 前缀；`idempotency_keys` 保留 24 小时；`sessions` 按过期时间清理。
- 关闭时执行 WAL checkpoint；异常退出后启动时检查数据库完整性并清理临时文件。

## 9. 事件与幂等

事件类型至少包括：

- `runtime.starting`、`runtime.running`、`runtime.stopping`、`runtime.stopped`、`runtime.crashed`、`runtime.failed`。
- `proxy.engine.failed`、`proxy.engine.rebuilt`、`proxy.check.completed`。
- `profile.created`、`profile.updated`、`profile.archived`、`profile.restored`、`profile.deleted`、`profile.purged`。
- `kernel.installed`、`engine.installed`、`audit.recorded`。

事件写入 `event_log` 后广播，`sequence` 单调递增。WebSocket 客户端以整数 `after=<sequence>` 重连；过期序号返回明确错误，客户端重新拉取快照。

写操作幂等键包含作用域、请求哈希和响应；相同键与相同请求返回原响应，相同键与不同请求返回 409，过期键重新执行。

## 10. 安全边界

- 核心不信任宿主输入；所有路径规范化并校验位于数据根内，拒绝符号链接穿越。
- argv 使用数组，不经过 shell；代理配置由产品生成，不直接透传用户 JSON。
- 引擎凭据只经回环 gRPC 管理 API 传递；服务端按 REQ-PROXY-017 采用 UID/LSM 或独立网络边界，桌面以回环绑定和页面探测补偿并记录同 UID 残余风险。秘密不写持久化配置、argv、日志、事件或 API。
- 内核与引擎安装时校验 `asset_sha256`，启动前校验 `binary_sha256`；所有宿主在资源创建前探测沙箱，桌面无法确认时拒绝，服务端仅在显式 `ALLOW_UNSANDBOXED=1` 且通过包含性探针与独立 UID/网络边界时降级。
- 服务端管理员密码以 Argon2id 存储，由环境变量或文件引导，首次登录强制改密；改密后仅删除应用自己生成的引导文件，外部环境变量/Secret 由部署者撤销。API Token 使用 HKDF-peppered 哈希，不可逆加密。
- 服务端 HTTP 写操作校验会话或 Token scope，Cookie 写操作另校验 Origin 与 CSRF；桌面 IPC 校验本机进程信任、档案归属与领域输入，不要求会话/Token。桌面自动化 HTTP 仍校验 Bearer scope。非回环服务端必须配置 `MIEN_PUBLIC_URL` 与 Secure Cookie；转发头只信任实际反代对端，限流使用校验后的客户端 IP。
- CDP、noVNC、运行档案入站与 Xray 管理 API 只绑定回环；管理 API 的服务端 UID/LSM 或网络边界及桌面补偿控制按 REQ-PROXY-017，独立内核 UID/等价网络边界是服务端无沙箱的强制条件，不冒充所有宿主的既有保证；桌面自动化拒绝非回环绑定。
- 渲染进程与无沙箱模式不得获得 `/data/secrets` 或数据库的访问权，也不得触达 Xray 管理 API；Chromium 命名空间网络隔离不保证宿主回环不可达，因此用页面发起的回环探测验证管理 API 不可达；子进程 `env_clear` 后只注入白名单变量。服务端直连档案未设置 `MIEN_ALLOW_DIRECT_EGRESS=1` 时拒绝启动并写稳定错误，设置后写审计事件。
- 事件与审计元数据统一脱敏；错误响应带 request_id，不带堆栈。

## 11. 宿主适配

### 11.1 传输抽象

管理网页不直接调用 `fetch` 或 `invoke`，而是通过领域服务调用统一的 `ControlTransport`。桌面与服务端只在适配器层分化。

```ts
export interface ControlTransport {
  command<T>(name: CommandName, args: unknown): Promise<T>;
  subscribe(handler: (event: DomainEvent) => void): Promise<() => void>;
}

export function createTransport(): ControlTransport {
  return isTauri() ? createTauriTransport() : createHttpTransport();
}
```

领域服务示例：

```ts
export const profilesApi = {
  list: (query: ListQuery) =>
    transport.command<ProfilePage>("profiles_list", { query }),
  create: (input: CreateProfile, idempotencyKey: string) =>
    transport.command<Profile>("profiles_create", { input, idempotencyKey }),
};
```

### 11.2 桌面 Tauri 适配器

桌面适配器使用 `@tauri-apps/api/core` 的 `invoke` 和 `@tauri-apps/api/event` 的 `listen`：

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const createTauriTransport = (): ControlTransport => ({
  command: (name, args) => invoke(name, args),
  subscribe: async (handler) => {
    const unlisten = await listen<DomainEvent>("mien://event", (event) =>
      handler(event.payload),
    );
    return unlisten;
  },
});
```

Rust 侧命令是薄适配器，直接调用核心用例：

```rust
#[tauri::command(rename_all = "camelCase")]
async fn profiles_list(
    state: tauri::State<'_, AppState>,
    query: ListQuery,
) -> Result<ProfilePage, ApiError> {
    state.profiles.list(query).await.map_err(ApiError::from)
}

tauri::Builder::default()
    .invoke_handler(tauri::generate_handler![
        profiles_list,
        profiles_create,
        runtime_start,
        runtime_stop,
        events_after,
    ]);
```

约定：

- 命令名使用 `<domain>_<action>`，由共享 `CommandName` 类型约束；命令参数通过 `rename_all` 明确固定。
- 核心 DTO 使用 `serde` 序列化，并通过 `ts-rs` 生成 TypeScript 类型；HTTP 侧用 `utoipa` 生成 OpenAPI。
- Tauri 命令返回 `Result<T, ApiError>`；错误结构与 HTTP 一致，包含稳定 `code`、`message`、`request_id`、`retryable` 和非敏感 `details`。
- 桌面 UI 不启动 `http://127.0.0.1` 控制面。自动化 HTTP API 是独立能力，默认关闭、只绑定回环、使用 Bearer Token。

### 11.3 事件与重载补齐

- 核心通过注入的 `EventSink` 先把事件写入 `event_log`；桌面宿主适配器订阅已持久化事件并调用 Tauri `Emitter`，核心不依赖 Tauri。
- UI 先订阅事件频道，再调用 `events_after(last_sequence)` 补齐；按 `event_id` 去重。
- WebView 刷新后从内存或 `sessionStorage` 恢复最后 `sequence`；没有游标时先拉取档案与运行时快照，再从最新事件继续。
- 服务端使用 `/api/v1/events?after=<sequence>` WebSocket（整数游标），复用同一事件 DTO 与去重逻辑。
- 事件频道只传 DTO，不传凭据、完整配置或目标 URL；敏感字段在核心侧已经脱敏。

### 11.4 文件、安全与开发模式

- 桌面导入导出使用 Tauri dialog 插件选择文件；服务端使用 `POST /api/v1/profiles/import/validate`（dry-run）、`POST /api/v1/profiles/import` 与 `POST /api/v1/profiles/{id}/export`（请求体携带 `format` 与可选 `passphrase`），都校验大小、schema 和符号链接。
- 运行时日志通过 `GET /api/v1/profiles/{id}/logs?limit=` 读取有界、脱敏的日志尾部；桌面走同一核心用例的本地适配，不开放 HTTP。
- WebView 不直接读取任意路径；外部链接通过 opener 插件交给系统浏览器，不在主 WebView 导航到远程页面。
- Tauri capabilities 只授权主窗口调用所需命令；生产 CSP 禁止远程脚本和内联脚本。
- 服务端适配器使用 `fetch` 与 `credentials: "same-origin"`，写操作带 CSRF Token；WebSocket 使用会话 Cookie 或一次性票据。
- 开发模式由 Vite dev server 提供页面，Tauri IPC 仍然可用；生产构建使用打包后的静态资源与严格 CSP。
- 合同测试覆盖：命令名与 DTO 一致、错误结构一致、事件回放、桌面默认无 HTTP 监听端口、远程内容不能获得 IPC 能力。

## 12. 测试缝

- `FakeProcessLauncher` 模拟启动、退出码、超时和强杀。
- `FakeEngine` 模拟动态入站/出站/规则、引用计数、API 失败、引擎崩溃和重建。
- `FakeClock` 固定时间，验证退避、过期和事件排序。
- 临时数据目录与临时 SQLite 文件，保证测试隔离。
- Golden fixture：指纹规格 → argv；代理配置 → 出站/入站/规则；错误 → HTTP 映射。
- 合同测试：OpenAPI、JSON Schema、事件 schema、导入导出 schema。
- 端到端：受控协议节点、DNS canary、WebRTC 探测、Docker 有头/无头/伪有头。

## 13. 部署契约

- macOS 应用包含内核与 Xray-core 二进制、MPL-2.0 许可与更新清单；数据目录不随应用卸载删除。
- Docker 镜像使用非 root 用户、只读根文件系统、`/data` 唯一持久可写卷；`--tmpfs /tmp:rw,noexec,nosuid,size=1g`，`/dev/shm` 显式定量，HOME/XDG 可写目录在 `/data` 下；入口脚本（tini 作为 PID 1）管理 Xvfb、openbox、可选 noVNC 和信号转发。
- 容器强制从宿主侧发布物加载沙箱 seccomp 配置（允许非特权用户命名空间的最小 allowlist），提供 `--security-opt seccomp=<host-path>` 的官方启动参数；`<host-path>` 必须是 Docker daemon 可见文件，不得指向镜像内审计副本；不得把 seccomp 当作可选加固。
- 管理员初始密码通过 `MIEN_ADMIN_PASSWORD` 或 `MIEN_ADMIN_PASSWORD_FILE` 引导，不写入镜像层；首次改密后删除应用生成的引导文件并清除标记，外部 Secret 由部署者撤销。
- 每档案下载目录为 `/data/profiles/<id>/downloads`（0700），并作为 Chromium 默认下载目录。
- Docker 端口：控制面 8080 映射；noVNC 6080 仅在启用查看器时映射；CDP、Xray gRPC 管理 API 与每运行档案 HTTP 入站永不映射。
- Hysteria2 需要容器允许 UDP 出站；不满足时明确报错，不回退其他协议；TUIC 不进入 v1。
- Apple Silicon 运行 `linux/amd64` 镜像时记录模拟性能；原生 ARM64 不在首个发布范围。

## 14. 版本与兼容

独立版本轴：

- API：`/api/v1`。
- 指纹规格：`schema_version`。
- 档案导出：`schema_version`。
- 内核能力清单：`kernel_version` + `platform`。
- 协议引擎能力清单：`version` + `platform`。
- 数据库：迁移版本。

升级规则：规格迁移保留未知字段或拒绝加载；内核/引擎升级先比对能力清单；运行中档案不静默替换；导出保留复现三要素（种子、内核版本、规格版本）。

## 15. 参考

- [`ARCHITECTURE.md`](ARCHITECTURE.md)
- [`TECH-STACK.md`](TECH-STACK.md)
- [`PRD.md`](PRD.md)
- [`SECURITY.md`](SECURITY.md)
- [`TEST-PLAN.md`](TEST-PLAN.md)
- [`specs/`](specs/)
- [`adr/`](adr/)
- [`DATABASE-SCHEMA.md`](DATABASE-SCHEMA.md)
- [`ERROR-CODES.md`](ERROR-CODES.md)
- [`contracts/openapi.yaml`](contracts/openapi.yaml)
