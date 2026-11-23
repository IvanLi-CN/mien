# Mien 错误码目录

本文固定控制面与运行时的稳定错误码。错误响应结构为：

```json
{
  "code": "PROXY_ENGINE_NOT_READY",
  "message": "The shared proxy engine is not ready.",
  "request_id": "01J...",
  "retryable": true,
  "details": {}
}
```

`code` 是稳定契约；`message` 是给人阅读的英文或本地化消息，不作为程序分支依据。`details` 只能包含字段级错误、资源标识和非敏感诊断；不得包含代理凭据、UUID、私钥、Cookie、Token、完整分享链接或目标 URL。

本文的代码集合就是 `ErrorCode` 枚举的契约。OpenAPI 的 `Error.code` 必须引用由本目录生成的 `ErrorCode` 枚举；事件 `error_code` 与测试 fixture 的取值必须是该集合成员（事件字段允许 `null`）。CI 双向比对本文与契约枚举，出现新增、删除或 HTTP 映射漂移即失败（见第 8 节）。

## 1. 认证与授权

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `AUTH_REQUIRED` | 401 | 否 | 缺少会话或 Bearer Token |
| `AUTH_INVALID` | 401 | 否 | 会话过期、Token 无效或已被撤销 |
| `AUTH_RATE_LIMITED` | 429 | 是 | 登录或认证尝试超过限流 |
| `AUTH_CSRF_INVALID` | 403 | 否 | Cookie 会话写操作缺少或使用了错误 CSRF Token |
| `AUTH_ORIGIN_INVALID` | 403 | 否 | `Origin`/`Referer` 不在允许列表；适用于 Cookie 写请求与 WebSocket 升级 |
| `FORBIDDEN_SCOPE` | 403 | 否 | Token scope 不允许该操作 |
| `AUTH_PASSWORD_CHANGE_REQUIRED` | 403 | 否 | 首次登录必须先修改初始密码 |
| `EVENT_TICKET_INVALID` | 401 | 否 | WebSocket 一次性票据缺失、过期或已使用 |

## 2. 资源与状态

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `PROFILE_NOT_FOUND` | 404 | 否 | 档案行不存在（含已永久删除） |
| `PROFILE_DELETED` | 409 | 否 | 档案处于软删除 `deleted` 状态，必须先 restore 才能启动或修改 |
| `PROFILE_PURGE_FAILED` | 500 | 是 | 永久删除时目录移除失败；行保留为 `deleted` 并写入 `deletion_error` 供重试 |
| `PROXY_NOT_FOUND` | 404 | 否 | 代理配置不存在 |
| `RUNTIME_NOT_FOUND` | 404 | 否 | 档案没有受监督运行时 |
| `TOKEN_NOT_FOUND` | 404 | 否 | API Token 不存在 |
| `PROFILE_ARCHIVED` | 409 | 否 | 档案已归档，不能执行该操作 |
| `RUNTIME_CONFLICT` | 409 | 否 | 同一档案已有其他启动/停止事务 |
| `IDEMPOTENCY_CONFLICT` | 409 | 否 | 相同幂等键携带了不同请求 |
| `RESOURCE_IN_USE` | 409 | 否 | 代理或引擎仍被运行档案引用 |
| `GROUP_NAME_CONFLICT` | 409 | 否 | 分组名称重复 |
| `TAG_NAME_CONFLICT` | 409 | 否 | 标签名称重复 |
| `PROFILE_IMPORT_CONFLICT` | 409 | 否 | 导入档案与现有档案冲突且未选择新 ID |
| `VIEWER_NOT_AVAILABLE` | 404 | 否 | 查看器未启用或运行时没有可查看显示；运行时行缺失时优先返回 `RUNTIME_NOT_FOUND` |

找不到资源的语义必须区分“行不存在”和“资源仍存在但不可操作”：软删除且未 purge 的档案返回 `PROFILE_DELETED`（409），不返回 404；已 purge 的档案与从未存在的 ID 返回 `PROFILE_NOT_FOUND`。查看器请求先检查档案与运行时，再检查查看器能力，不得用 `VIEWER_NOT_AVAILABLE` 掩盖缺失的运行时。

`tokensCreate` 接受 `Idempotency-Key`，但明文 Token 永远不重放：重复键必须返回 `IDEMPOTENCY_CONFLICT`（409），不得返回缓存的 201 响应。

## 3. 输入与能力

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `VALIDATION_FAILED` | 422 | 否 | 通用请求字段错误；`details.fields` 给出字段级原因 |
| `FINGERPRINT_INVALID` | 422 | 否 | 指纹规格不一致或超出内核能力清单 |
| `PROXY_CONFIG_INVALID` | 422 | 否 | 代理类型、传输、TLS 或凭据字段无效 |
| `SHARE_LINK_INVALID` | 422 | 否 | 分享链接格式、协议或参数不受支持 |
| `RUNTIME_MODE_UNSUPPORTED` | 422 | 否 | 当前内核或宿主不支持所选运行模式 |
| `KERNEL_UNSUPPORTED` | 422 | 否 | 请求的字段或组合不在内核能力清单中 |
| `UNSUPPORTED_PROTOCOL` | 422 | 否 | 当前引擎能力清单不支持该协议或传输 |
| `TUIC_NOT_SUPPORTED` | 422 | 否 | v1 明确不支持 TUIC |

## 4. 配额与资源

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `QUOTA_EXCEEDED` | 429 | 是 | 并发运行档案数达到上限 |
| `INSUFFICIENT_RESOURCES` | 503 | 是 | 内存、磁盘或文件描述符不足 |
| `KERNEL_NOT_FOUND` | 500 | 否 | 已安装内核的可执行文件缺失 |
| `KERNEL_CHECKSUM_MISMATCH` | 500 | 否 | 内核发布资产或可执行文件摘要不符 |
| `PROXY_ENGINE_NOT_FOUND` | 500 | 否 | 已安装 Xray 的可执行文件缺失 |
| `PROXY_ENGINE_CHECKSUM_MISMATCH` | 500 | 否 | Xray 发布资产或可执行文件摘要不符 |

安装记录同时保存 `asset_sha256`（下载的发布资产）与 `binary_sha256`（解包后可执行文件）。安装时校验 `asset_sha256`，运行时只校验 `binary_sha256`；文件缺失与摘要不符是两类不同错误，不得都用 `*_CHECKSUM_MISMATCH`。

## 5. 代理引擎

| 错误码 | 返回位置 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- | --- |
| `PROXY_ENGINE_NOT_READY` | 错误响应 | 503 | 是 | 共享引擎尚未启动或 API 尚未就绪 |
| `PROXY_ENGINE_API_UNAVAILABLE` | 错误响应 | 503 | 是 | Xray 管理 API 连接失败 |
| `PROXY_ENGINE_START_FAILED` | 错误响应 | 500 | 否 | 共享引擎无法启动 |
| `PROXY_ENGINE_CRASHED` | 错误响应、事件 | 503 | 是 | 共享引擎异常退出，正在重启 |
| `PROXY_INBOUND_CONFLICT` | 错误响应 | 409 | 是 | 运行档案入站端口或标签已被占用 |
| `PROXY_OUTBOUND_REFCOUNT_MISMATCH` | 错误响应 | 500 | 否 | 共享出站引用计数对账失败 |
| `PROXY_CONNECTION_REFUSED` | `ProxyCheck`、事件 | 200 | 是 | 节点拒绝连接或端口不可达 |
| `PROXY_AUTH_FAILED` | `ProxyCheck`、事件 | 200 | 否 | 节点认证失败 |
| `PROXY_TLS_FAILED` | `ProxyCheck`、事件 | 200 | 否 | TLS/REALITY 握手或证书校验失败 |
| `PROXY_TIMEOUT` | `ProxyCheck`、事件 | 200 | 是 | 连接或请求超时 |
| `PROXY_CONNECTION_FAILED` | `ProxyCheck`、事件 | 200 | 是 | 未归类的连接失败；不得替代上述稳定分类 |
| `PROXY_DNS_FAILED` | `ProxyCheck`、事件 | 200 | 是 | 目标域名或节点解析失败 |

代理检查（`POST /api/v1/proxies/{id}/check` 与分享链接检查）成功与失败都以 HTTP 200 返回 `ProxyCheck`：`ok=false` 时网络错误放在 `error_code`，`message` 为脱敏说明，延迟、出口 IP 和地区字段可为 null；事件 `proxy.check.completed` 携带同一 `error_code`。节点网络失败不属于控制面 HTTP 错误，不得用未声明的 502 表达。只有认证、授权、校验、存储、引擎未就绪等控制面失败才使用 HTTP 错误响应；检查未通过只是启动前警告，不阻断启动，因此不提供单独的“检查未就绪”错误码。

## 6. 运行时

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `RUNTIME_SPAWN_FAILED` | 500 | 否 | 内核进程无法创建 |
| `RUNTIME_CDP_TIMEOUT` | 500 | 是 | CDP 在超时内未就绪 |
| `SANDBOX_UNAVAILABLE` | 500 | 否 | Docker 中无法确认 Chromium 沙箱 |
| `DISPLAY_UNAVAILABLE` | 503 | 是 | 有头模式所需 Xvfb/宿主显示不可用 |

`SANDBOX_UNAVAILABLE` 在创建代理资源与启动进程之前返回；显式 `ALLOW_UNSANDBOXED=1` 时启动继续，但运行时 `isolation=unsandboxed` 且必须写审计事件。


## 7. 存储与内部

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `STORAGE_BUSY` | 503 | 是 | SQLite 忙或短时写锁冲突 |
| `STORAGE_MIGRATION_FAILED` | 500 | 否 | 数据库迁移失败且需要人工处理 |
| `STORAGE_INTEGRITY_FAILED` | 500 | 否 | 数据完整性检查失败 |
| `INTERNAL_UNEXPECTED` | 500 | 否 | 未分类错误；必须带 `request_id`，不得返回堆栈 |

## 8. 规则

- 新增错误码必须同时更新本文、OpenAPI、事件 schema 的相关枚举和测试。
- 错误码一旦发布，只能弃用，不能改变语义后复用。
- 代理凭据、Cookie、Token、UUID、私钥和完整分享链接永远不能进入 `message` 或 `details`。
- 可重试错误必须返回 `retryable=true`；客户端按指数退避重试，并设置上限。
- UI 必须能根据 `code` 区分配置、引擎、节点和目标站点问题。
- CI 必须同时校验：本文代码集合与 OpenAPI `Error.code` 枚举双向一致；每个代码的 HTTP 与 `retryable` 映射一致；事件 `error_code` 与 fixture 取值经契约测试验证属于该集合（允许 `null`）；生成物 `git diff --exit-code` 无漂移。只允许第 5 节列出的代理检查错误码在 HTTP 200 响应体中携带 `error_code`。
- 每个错误码至少有一个 `fixtures/api/errors/` 响应样例；`ProxyCheck` 错误还要有 `ok=false` 的 200 响应样例。没有样例或样例与目录不一致的代码不得发布。
- 代码不得只存在于实现或 OpenAPI：目录是契约来源，枚举由目录生成或逐项校验；发现目录外代码、未使用代码或未声明状态码即构建失败。
