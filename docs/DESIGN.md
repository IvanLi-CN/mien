# Snailfish 详细设计

## 1. 目的与边界

本文把 [`ARCHITECTURE.md`](ARCHITECTURE.md) 的系统级设计落到可实现的模块、接口、并发、错误、配置和存储约束。技术选型与替代方案见 [`TECH-STACK.md`](TECH-STACK.md)；长期能力契约见 [`specs/`](specs/)。本文不重复产品需求、ADRs 的完整取舍或测试矩阵。

## 2. 模块与依赖方向

```text
snailfish-desktop ─┐
snailfish-server  ─┼─> snailfish-core
snailfish-cli     ─┘
```

- `snailfish-core` 不依赖 Tauri、axum、React 或具体数据库驱动；领域层只依赖标准库、`serde`、`time` 和轻量工具 crate。
- `snailfish-core` 内部按领域模块组织：`profile`、`fingerprint`、`proxy`、`runtime`、`kernel`、`engine`、`storage`、`security`、`events`。
- 端口（trait）定义在核心内，适配器在核心的 `adapters` 或宿主 crate 中实现。
- 宿主只做 DTO 转换、认证、静态资源、窗口和平台集成；业务规则不得在宿主复制。

依赖规则：

- 领域层不得直接访问文件系统、进程、网络、钥匙串或 SQLite。
- 适配器不得反向依赖宿主框架。
- 所有跨层数据使用显式 DTO 或领域类型，禁止把 axum/Tauri 类型传入核心。
- 时间、随机数、进程启动、密钥读取和事件发布必须通过 trait 注入，保证测试可替代。

## 3. 领域类型与端口

### 3.1 标识与状态

- `ProfileId(Ulid)`、`RuntimeId(Ulid)`、`KernelVersion`、`EngineVersion`。
- `SpecSchema`、`KernelManifest`、`EngineManifest` 各自独立版本。
- `RuntimeState`：`Stopped`、`Starting`、`Running`、`Stopping`、`Crashed`、`Failed`。
- `ProxyConfig`：`Http`、`Https`、`Socks5`、`Vless`、`Vmess`、`Shadowsocks`、`Trojan`、`Hysteria2`、`Tuic`、`Wireguard` 的 tagged enum；秘密字段使用 `SecretString`，`Debug` 输出必须脱敏。
- `LaunchPlan`：内核 argv、环境变量、用户数据目录、代理入口、是否启用 CDP、日志目标。
- `RuntimeHandle`：运行时标识、档案标识、内核 PID、引擎 PID、代理端口、CDP 端口、取消令牌。

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

- `KernelRegistry`：解析版本与平台、校验 SHA-256、读取内核能力清单。
- `EngineRegistry`：解析协议引擎版本与平台、校验 SHA-256、读取协议能力清单与许可信息。
- `FingerprintValidator`：schema、一致性、能力清单校验，输出规范化规格。
- `ProxyValidator`：协议、传输、TLS/REALITY、凭据字段、能力清单校验，输出可生成的引擎配置。
- `BuildArgv`：纯函数，输入规格与运行时参数，输出确定性参数数组。
- `BuildEngineConfig`：纯函数，输入代理配置、端口与路径策略，输出引擎配置对象；写盘与启动由运行时负责。

## 4. 启动与停止事务

### 4.1 启动顺序

1. 宿主完成认证与授权，调用核心启动用例。
2. 读取档案；拒绝已归档、已删除或正在删除的档案。
3. 获取档案级 actor 锁；已有启动/停止操作时按幂等语义返回。
4. 获取全局运行时配额；检查可用内存、磁盘和并发上限。
5. 校验指纹规格、代理配置、内核与协议引擎能力清单。
6. 校验内核与引擎二进制 SHA-256；失败则不创建任何临时资源。
7. 分配回环代理端口；生成引擎配置到运行时临时目录，权限 0600。
8. 执行引擎配置校验并启动引擎；等待 mixed 入站就绪。
9. 生成内核 argv，注入 `--proxy-server=http://127.0.0.1:<proxy_port>`、数据目录、CDP 和显示参数。
10. 启动内核；等待 `DevToolsActivePort` 与 CDP 探测（如启用）。
11. 持久化 `runtimes=running`，发布 `runtime.running`，返回运行时句柄。
12. 删除临时引擎配置；运行时只保留加密数据库中的代理配置。

失败回滚顺序与启动相反：终止内核进程组、停止引擎、删除临时配置、释放端口、释放配额、释放 actor 锁、写入 `failed` 与稳定错误码。回滚中的二次失败必须记录，但不得掩盖原始错误。

### 4.2 停止顺序

1. 状态改为 `stopping`，发布事件。
2. 向内核发送正常退出信号，等待数据落盘。
3. 超时后终止内核进程组。
4. 停止协议引擎；超时后终止引擎进程组。
5. 删除临时配置与端口占用，释放配额与 actor 锁。
6. 写入 `stopped`，发布事件；强制终止写入审计事件。

停止对已停止档案幂等返回成功。停止失败时保留 `runtimes` 记录和诊断信息，允许重试清理。

### 4.3 重启对账

控制面启动时读取非终态 `runtimes`：

- 通过 PID、进程启动时间、命令行和数据目录确认内核与引擎归属。
- 可确认归属且支持接管时恢复监控；否则安全终止并标记 `crashed` 或 `stopped`。
- 无法确认归属的进程不盲目杀死，标记为孤儿并按 TTL 清理。
- 删除过期临时配置、锁文件和端口引用；未完成操作写入失败事件。

## 5. 并发与锁

- 每个档案一个 actor，命令通过 `mpsc` 串行化；启动/停止不会并发执行。
- 全局 `Semaphore` 控制最大运行档案数；资源检查在获取配额后进行，失败立即释放。
- SQLite 使用连接池，事务短小；禁止在持有数据库事务时等待进程、网络或引擎就绪。
- 锁顺序固定为：档案 actor → 全局配额 → 代理端口 → 引擎 → 内核；反向释放。
- 事件持久化与广播通过独立任务解耦；广播无订阅者不影响事件落库。
- 进程等待使用异步子进程 API；文件哈希、目录清理等可能阻塞的操作放入阻塞线程池。
- 关闭时设置全局取消令牌，按启动逆序停止运行时，最后执行 WAL checkpoint。

## 6. 错误模型

核心返回 `DomainError { code, message, retryable, details }`。宿主映射为 HTTP 或 Tauri 错误；`details` 只能包含非敏感字段。

| 前缀 | 示例 | HTTP | 说明 |
| --- | --- | --- | --- |
| `AUTH_` | `AUTH_REQUIRED`、`AUTH_INVALID` | 401 | 缺失、过期或无效会话/Token |
| `FORBIDDEN_` | `FORBIDDEN_SCOPE` | 403 | 已认证但无权限 |
| `NOT_FOUND_` | `PROFILE_NOT_FOUND` | 404 | 资源不存在 |
| `CONFLICT_` | `PROFILE_ARCHIVED`、`RUNTIME_CONFLICT` | 409 | 状态冲突或幂等键冲突 |
| `VALIDATION_` | `FINGERPRINT_INVALID`、`PROXY_CONFIG_INVALID` | 422 | 输入不可用 |
| `QUOTA_` | `QUOTA_EXCEEDED` | 429 | 并发或资源不足 |
| `KERNEL_` | `KERNEL_CHECKSUM_MISMATCH` | 500/409 | 内核缺失、损坏或版本不兼容 |
| `PROXY_` | `PROXY_ENGINE_CHECKSUM_MISMATCH`、`PROXY_ENGINE_NOT_READY` | 502/503 | 引擎或节点失败 |
| `RUNTIME_` | `RUNTIME_CDP_TIMEOUT`、`RUNTIME_SPAWN_FAILED` | 500 | 进程生命周期失败 |
| `STORAGE_` | `STORAGE_BUSY`、`STORAGE_MIGRATION_FAILED` | 500 | 数据库或文件系统失败 |
| `INTERNAL_` | `INTERNAL_UNEXPECTED` | 500 | 未分类错误，必须带 request_id |

错误消息不得包含代理凭据、UUID、私钥、完整分享链接、Cookie、Token 或完整启动参数。可重试错误必须返回 `retryable=true` 和重试建议。

## 7. 配置矩阵

配置优先级从高到低：CLI 参数、环境变量、数据库设置、平台默认值。秘密只允许来自钥匙串、环境变量或 0600 文件，不得写入数据库设置。

| 变量 | 默认 | 作用 |
| --- | --- | --- |
| `SNAILFISH_DATA_DIR` | 平台数据目录或 `/data` | 数据库、档案、内核、引擎和日志根 |
| `SNAILFISH_BIND` | 服务端 `0.0.0.0:8080`；桌面回环随机端口 | 控制面监听地址 |
| `SNAILFISH_MASTER_KEY` | 无 | 服务端主密钥；未提供时使用 `/data/secrets/master.key` |
| `SNAILFISH_SCREEN` | `1920x1080x24` | Docker Xvfb 分辨率 |
| `SNAILFISH_MAX_RUNTIMES` | `10` | 最大并发运行档案数 |
| `SNAILFISH_LOG` | `info` | 日志级别与格式 |
| `ALLOW_UNSANDBOXED` | `false` | 是否允许无沙箱内核；必须显式开启并审计 |
| `SNAILFISH_VIEWER` | `false` | 是否启用 noVNC 查看器 |
| `SNAILFISH_UPDATE_CHECK` | `false` | 是否检查更新；默认不联网 |

macOS 使用 `~/Library/Application Support/Snailfish` 与 `~/Library/Logs/Snailfish`；Docker 使用 `/data` 与标准输出日志。

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
- `proxy.engine.failed`、`proxy.check.completed`。
- `profile.created`、`profile.updated`、`profile.archived`、`profile.deleted`。
- `kernel.installed`、`engine.installed`、`audit.recorded`。

事件写入 `event_log` 后广播，标识单调递增。WebSocket 客户端以最后标识重连；过期标识返回明确错误，客户端重新拉取快照。

写操作幂等键包含作用域、请求哈希和响应；相同键与相同请求返回原响应，相同键与不同请求返回 409，过期键重新执行。

## 10. 安全边界

- 核心不信任宿主输入；所有路径规范化并校验位于数据根内，拒绝符号链接穿越。
- argv 使用数组，不经过 shell；代理配置由产品生成，不直接透传用户 JSON。
- 引擎配置 0600，启动后删除；秘密不进入进程参数、日志、事件或 API。
- 内核与引擎启动前校验 SHA-256；Docker 拒绝未验证沙箱的无沙箱启动，除非显式开启 `ALLOW_UNSANDBOXED`。
- 控制面写操作校验会话/Token、范围、Origin 与 CSRF；CDP、noVNC 和 mixed 入站只绑定回环。
- 事件与审计元数据统一脱敏；错误响应带 request_id，不带堆栈。

## 11. 宿主适配

- 服务端：axum 接收 DTO，认证后调用核心用例；WebSocket 从 `event_log` 回放；静态 UI 与 API 同源；`/healthz`、`/readyz` 分离。
- 桌面：Tauri 命令调用同一核心用例；钥匙串实现 `SecretStore`；默认不开放 HTTP 控制面；自动化 API 显式启用后只绑定回环。
- CLI：迁移、诊断、内核与引擎准入辅助；不实现独立业务规则。
- 宿主错误映射到统一错误结构；日志注入 request_id、profile_id、runtime_id。

## 12. 测试缝

- `FakeProcessLauncher` 模拟启动、退出码、超时和强杀。
- `FakeEngine` 模拟配置校验、就绪、失败和节点不可达。
- `FakeClock` 固定时间，验证退避、过期和事件排序。
- 临时数据目录与临时 SQLite 文件，保证测试隔离。
- Golden fixture：指纹规格 → argv；代理配置 → 引擎配置；错误 → HTTP 映射。
- 合同测试：OpenAPI、JSON Schema、事件 schema、导入导出 schema。
- 端到端：受控协议节点、DNS canary、WebRTC 探测、Docker 有头/无头。

## 13. 部署契约

- macOS 应用包含内核与 sing-box 二进制、许可与更新清单；数据目录不随应用卸载删除。
- Docker 镜像使用非 root 用户、只读根文件系统、`/data` 唯一可写卷；入口脚本管理 Xvfb、openbox、可选 noVNC 和信号转发。
- Docker 端口：控制面 8080、noVNC 6080（默认关闭）、CDP 与 mixed 入站不映射。
- Hysteria2/TUIC 需要容器允许 UDP 出站；不满足时明确报错，不回退其他协议。
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
