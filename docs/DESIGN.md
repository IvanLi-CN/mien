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
- 合法转移：`Stopped → Starting`、`Starting → Running|Failed|Crashed`、`Running → Stopping|Crashed|Failed`、`Stopping → Stopped`、`Crashed → Starting|Stopped`、`Failed → Stopped`。`Running → Failed` 只用于共享引擎故障且 teardown 已完成；`Starting → Crashed` 用于内核提前退出。任何终态都不得存在活内核、入站、规则或计入引用计数的出站。
- `ProfileStatus`：`Active`、`Archived`、`Deleted`，对外序列化为 `active`/`archived`/`deleted`；`Deleted` 必须携带 `deleted_at`，永久删除失败时携带 `deletion_error` 并保持 `Deleted`，可重试；`Deleted` 不能启动。
- `Isolation`：`Enforced`、`Unsandboxed`、`Unknown`，对外序列化为 `enforced`、`unsandboxed`、`unknown`；每次启动由沙箱探测写入 `runtimes.isolation`，是 proof strip 的唯一数据源。
- `SandboxKind`：`Namespace`、`Seatbelt`、`AppContainer`、`None`，来自内核能力清单 `sandbox_kind`。
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
6. 校验内核与引擎可执行文件的 `binary_sha256`（不是清单资产摘要）；失败则不创建任何临时资源。
7. 在事务内写入 `runtimes(state=starting, isolation=unknown)` 并发布 `runtime.starting`；此后任何失败都能被对账发现。
8. 沙箱验证：按内核清单 `sandbox_kind` 探测实际生效状态，得出 `isolation=enforced|unsandboxed|unknown`；Docker 中无法确认且未启用 `ALLOW_UNSANDBOXED=1` 时返回 `SANDBOX_UNAVAILABLE`，不创建代理资源、不启动进程；桌面无法确认时不阻断，但写 `unsandboxed` 或 `unknown`。通过后立即持久化 `isolation`。
9. 代理模式：分配回环代理端口；确保共享 Xray 引擎运行并连接 gRPC 管理 API；断言 blackhole 是第一个出站；计算 `outbound_key`，添加或复用出站并增加引用计数；添加带稳定 `ruleTag` 的入站和路由（`shouldAppend=true`）。直接模式跳过本步骤。
10. 生成内核 argv；代理模式注入 `--proxy-server=http://127.0.0.1:<proxy_port>`，直接模式不注入代理参数；保留 DoH/IPv6/UDP 防护参数；子进程 `env_clear` 后只注入白名单环境。
11. 启动内核；自动化开启时等待 `DevToolsActivePort`，关闭时等待进程存活、代理入站就绪（如适用）和有界启动宽限。
12. 持久化 `runtimes=running` 与 `isolation`，发布 `runtime.running`，返回运行时句柄。

失败回滚顺序与启动相反：终止内核进程组、按 `ruleTag` 删除路由规则与入站、清空 `proxy_*` 字段、递减非终态出站引用、释放端口、释放配额、释放 actor 锁、写入 `failed` 与稳定错误码。沙箱失败发生在资源创建之前，因此只需释放配额与锁。共享引擎本身不因单个运行时失败而退出。回滚中的二次失败必须记录，但不得掩盖原始错误；teardown 必须幂等且只执行一次。

### 4.2 停止顺序

1. 状态改为 `stopping`，发布事件。
2. 向内核发送正常退出信号，等待数据落盘。
3. 超时后终止内核进程组。
4. 按 `ruleTag` 删除该运行时的路由规则与入站。
5. 清空该行的 `proxy_outbound_key`、`proxy_inbound_tag`、`proxy_rule_tag`、`proxy_port`。
6. 递减非终态出站引用；引用归零时删除出站。
7. 释放端口、配额与 actor 锁；共享引擎保持运行。
8. 写入 `stopped`，发布事件；强制终止写入审计事件。

停止对已停止档案幂等返回成功。停止失败时保留 `runtimes` 记录和诊断信息，允许重试清理；重试仍从步骤 4 开始，不重复终止已退出进程。

### 4.3 重启对账

控制面启动时读取非终态 `runtimes`：

- 通过 PID、进程启动时间和 API 端口确认共享 Xray 引擎；通过 PID、命令行和数据目录确认各内核。
- 对 `starting`/`stopping` 中断行：终止可归属的内核进程组，按 `ruleTag` 删除入站与规则，清空 `proxy_*` 并递减非终态引用计数，然后写 `stopped`；只有 teardown 本身失败才写 `failed`。
- 引擎可接管时通过 gRPC 管理 API 对账入站/出站/规则；不可接管时重启引擎并按 `proxy_port` 重建全部运行配置（原 `ruleTag`、`shouldAppend=true`）。
- 共享引擎重建失败时，对每个受影响运行时执行第 4.1 节的同一 teardown，完成后才写 `failed`；不允许终态背后存在活内核或仍被计数的出站。
- 无法确认归属的进程不盲目杀死，标记为孤儿并按 TTL 清理。
- 删除过期锁文件和端口引用；未完成操作在 teardown 完成后以 `stopped` 或 `failed` 事件告知 UI，不留 `starting`/`stopping` 残留。

## 5. 并发与锁

- 每个档案一个 actor，命令通过 `mpsc` 串行化；启动/停止不会并发执行。
- 全局 `Semaphore` 控制最大运行档案数；资源检查在获取配额后进行，失败立即释放。
- SQLite 使用连接池，事务短小；禁止在持有数据库事务时等待进程、网络或引擎就绪。
- 锁顺序固定为：档案 actor → 全局配额 → 共享引擎写锁 → 代理端口/入站 → 内核；反向释放。引擎 API 操作必须串行化，避免重复出站或引用计数漂移。
- 事件持久化与广播通过独立任务解耦；广播无订阅者不影响事件落库。
- 进程等待使用异步子进程 API；文件哈希、目录清理等可能阻塞的操作放入阻塞线程池。
- 关闭时设置全局取消令牌，按启动逆序停止运行时，最后执行 WAL checkpoint。

## 6. 错误模型

核心返回 `DomainError { code, message, retryable, details }`。宿主映射为 HTTP 或 Tauri 错误；`details` 只能包含非敏感字段。

| 错误族 | 示例 | HTTP | 说明 |
| --- | --- | --- | --- |
| 认证与授权 | `AUTH_REQUIRED`、`AUTH_INVALID`、`AUTH_RATE_LIMITED`、`AUTH_PASSWORD_CHANGE_REQUIRED`、`AUTH_ORIGIN_INVALID`、`EVENT_TICKET_INVALID`、`FORBIDDEN_SCOPE` | 401/403/429 | 缺失、过期或无效凭据；`Origin` 不被信任；已认证但无权限或必须先改密 |
| 资源与生命周期 | `PROFILE_NOT_FOUND`、`PROXY_NOT_FOUND`、`RUNTIME_NOT_FOUND`、`TOKEN_NOT_FOUND`、`VIEWER_NOT_AVAILABLE`、`PROFILE_ARCHIVED`、`PROFILE_DELETED`、`PROFILE_PURGE_FAILED`、`RUNTIME_CONFLICT`、`RESOURCE_IN_USE`、`IDEMPOTENCY_CONFLICT`、`PROFILE_IMPORT_CONFLICT`、`GROUP_NAME_CONFLICT`、`TAG_NAME_CONFLICT` | 404/409/500 | `*_NOT_FOUND` 为 404；状态、归属、幂等键或命名冲突为 409；软删除未 purge 的档案返回 `PROFILE_DELETED` 而非 404；purge 目录删除失败为可重试 500 |
| 输入与能力 | `VALIDATION_FAILED`、`FINGERPRINT_INVALID`、`PROXY_CONFIG_INVALID`、`SHARE_LINK_INVALID`、`RUNTIME_MODE_UNSUPPORTED`、`KERNEL_UNSUPPORTED`、`UNSUPPORTED_PROTOCOL`、`TUIC_NOT_SUPPORTED` | 422 | `*_INVALID`、`*_UNSUPPORTED`、`*_NOT_SUPPORTED` 表达输入或能力不符 |
| 配额与资源不足 | `QUOTA_EXCEEDED`、`INSUFFICIENT_RESOURCES` | 429/503 | 并发上限可重试；内存、磁盘、描述符不足为 503 |
| 内核 | `KERNEL_NOT_FOUND`、`KERNEL_CHECKSUM_MISMATCH`、`KERNEL_UNSUPPORTED` | 500/422 | 文件缺失或 `binary_sha256` 不符；不兼容组合在启动前拒绝 |
| 内核宿主环境 | `SANDBOX_UNAVAILABLE`、`DISPLAY_UNAVAILABLE` | 500/503 | Docker 中沙箱无法确认时拒绝启动；显示环境不可用可重试 |
| 代理引擎 | `PROXY_ENGINE_NOT_FOUND`、`PROXY_ENGINE_CHECKSUM_MISMATCH`、`PROXY_ENGINE_START_FAILED`、`PROXY_ENGINE_NOT_READY`、`PROXY_ENGINE_API_UNAVAILABLE`、`PROXY_ENGINE_CRASHED`、`PROXY_INBOUND_CONFLICT`、`PROXY_OUTBOUND_REFCOUNT_MISMATCH` | 500/503/409 | 引擎安装、启动、API、端口与引用计数失败 |
| 代理节点 | `PROXY_CONNECTION_REFUSED`、`PROXY_AUTH_FAILED`、`PROXY_TLS_FAILED`、`PROXY_TIMEOUT`、`PROXY_DNS_FAILED` | 200 | 节点侧失败一律出现在代理检查的 200 `ProxyCheck.error_code` 与事件中，不用 HTTP 502；引擎未就绪由 `PROXY_ENGINE_NOT_READY` 控制面 503 表达 |
| 运行时 | `RUNTIME_SPAWN_FAILED`、`RUNTIME_CDP_TIMEOUT`、`RUNTIME_MODE_UNSUPPORTED` | 500/422 | 进程创建或就绪失败；模式不支持在启动前拒绝 |
| 存储 | `STORAGE_BUSY`、`STORAGE_MIGRATION_FAILED`、`STORAGE_INTEGRITY_FAILED` | 503/500 | 忙碌可重试；迁移与完整性失败需人工处理 |
| 内部 | `INTERNAL_UNEXPECTED` | 500 | 未分类错误，必须带 request_id |

前缀表示错误所属的名词族，不表示 HTTP 状态；状态由后缀决定：`*_NOT_FOUND` 为 404，`*_INVALID`/`*_UNSUPPORTED`/`*_NOT_SUPPORTED` 为 422，状态与幂等冲突为 409。错误码目录在 [`ERROR-CODES.md`](ERROR-CODES.md)，实现以目录与 OpenAPI 枚举为准；CI 校验目录、枚举和代码三方一致。代理检查的节点错误不做 HTTP 502：`POST /api/v1/proxies/{id}/check` 返回 200，失败写在 `ProxyCheck.error_code` 并同时发事件；HTTP 映射另行在契约文档定义。

错误消息不得包含代理凭据、UUID、私钥、完整分享链接、Cookie、Token 或完整启动参数。可重试错误必须返回 `retryable=true` 和重试建议。

## 7. 配置矩阵

配置优先级从高到低：CLI 参数、环境变量、数据库设置、平台默认值。秘密只允许来自钥匙串、环境变量或 0600 文件，不得写入数据库设置。

| 变量 | 默认 | 作用 |
| --- | --- | --- |
| `MIEN_DATA_DIR` | 平台数据目录或 `/data` | 数据库、档案、内核、引擎和日志根 |
| `MIEN_BIND` | 服务端 `0.0.0.0:8080`；桌面未设置 | 服务端控制面监听地址；桌面不监听 |
| `MIEN_DESKTOP_AUTOMATION_BIND` | 未设置 | 桌面可选自动化地址；显式设置后才监听，且必须是 `127.0.0.1`/`::1`，非回环值启动失败 |
| `MIEN_MASTER_KEY` | 无 | 服务端主密钥；未提供时使用 `/data/secrets/master.key` |
| `MIEN_ADMIN_PASSWORD` | 无 | 首次启动管理员密码引导值；与 `MIEN_ADMIN_PASSWORD_FILE` 二选一；两者都未提供时生成密码写入 `/data/secrets/admin-bootstrap` |
| `MIEN_ADMIN_PASSWORD_FILE` | 无 | 以 0600 文件提供管理员密码；首次改密后删除该文件与引导标记 |
| `MIEN_PUBLIC_URL` | 无 | 服务端对外基址；用于生成绝对链接与校验 `Origin` |
| `MIEN_TRUSTED_PROXIES` | 空 | 可信反向代理 CIDR/地址列表；仅这些来源的 `X-Forwarded-*` 被采信 |
| `MIEN_SCREEN` | `1920x1080x24` | Docker Xvfb 分辨率 |
| `MIEN_MAX_RUNTIMES` | `10` | 最大并发运行档案数 |
| `MIEN_LOG` | `info` | 日志级别与格式 |
| `ALLOW_UNSANDBOXED` | `false` | 是否允许无沙箱内核；必须显式开启并审计，开启后启动写入 `isolation=unsandboxed` |
| `MIEN_VIEWER` | `false` | 是否启用 noVNC 查看器 |
| `MIEN_UPDATE_CHECK` | `false` | 是否检查更新；默认不联网 |

macOS 使用 `~/Library/Application Support/Mien` 与 `~/Library/Logs/Mien`；Docker 使用 `/data` 与标准输出日志。

## 8. 存储与迁移

- SQLite 开启 WAL、外键、`busy_timeout=5000`；写入使用短事务。
- 迁移正向执行、版本化、不可变；不提供自动降级。迁移前创建数据库备份，失败时保持原库可打开。
- 迁移文件只增不改；开发期变更通过新迁移完成。
- 敏感字段以 AES-256-GCM 加密；密文包含版本、nonce 和 AAD（记录标识 + 字段名）。
- `event_log` 按 7 天或 100 MB 清理；`idempotency_keys` 保留 24 小时；`sessions` 按过期时间清理。
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
- 引擎凭据只经回环 gRPC 管理 API 传递，不写入持久化配置；秘密不进入进程参数、日志、事件或 API。
- 内核与引擎安装时校验 `asset_sha256`，启动前校验 `binary_sha256`；Docker 在资源创建前探测沙箱，无法确认就拒绝，除非显式开启 `ALLOW_UNSANDBOXED`。
- 服务端管理员密码以 Argon2id 存储在 `admin_credentials`，由 `MIEN_ADMIN_PASSWORD`/`MIEN_ADMIN_PASSWORD_FILE` 引导，首次登录强制改密，改密后删除引导来源；API Token 只用 HKDF-peppered 哈希，不用可逆加密。
- 控制面写操作校验会话/Token、范围、Origin 与 CSRF；服务端模式 Cookie 恒为 `Secure`；只有 `MIEN_TRUSTED_PROXIES` 中声明的来源才可提供 `X-Forwarded-*`，限流按校验后的客户端 IP。
- CDP、noVNC、运行档案 HTTP 入站和 Xray gRPC 管理 API 只绑定回环；桌面自动化 API 拒绝非回环绑定。
- 渲染进程与无沙箱模式不得获得 `/data/secrets` 或数据库的访问权，也不得触达 Xray 管理 API；子进程 `env_clear` 后只注入白名单变量。
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
- 容器强制加载随发布物交付的沙箱 seccomp 配置（允许非特权用户命名空间的最小 allowlist），提供 `--security-opt seccomp=<profile>` 的官方启动参数；不得把 seccomp 当作可选加固。
- 管理员初始密码通过 `MIEN_ADMIN_PASSWORD` 或 `MIEN_ADMIN_PASSWORD_FILE` 引导，不写入镜像层；首次改密后删除引导来源。
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
