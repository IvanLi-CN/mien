# 控制面 API 与鉴权

> 本文件是控制面主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: 桌面与服务端共享档案、指纹、代理和运行时能力，但宿主认证模型不同。服务端需要登录、会话和自动化 Token；桌面默认本地单用户，可通过可选回环 API 暴露自动化入口。
- In scope: REST 与 WebSocket 契约、鉴权、授权范围、CSRF、错误结构、分页、幂等、事件、健康检查、请求限制、审计、静态 UI 安全头。
- Out of scope: 指纹语义（由 `fingerprint-engine` 负责）、运行时进程细节（由 `browser-runtime` 负责）、代理协议（由 `proxy-gateway` 负责）。

## Terms and Interfaces

- `session`: 服务端登录后创建的浏览器会话，使用 `HttpOnly`、`SameSite=Strict` Cookie。
- `API token`: 自动化客户端使用的 Bearer Token，只保存哈希，带授权范围。
- `scope`: Token 允许执行的操作集合，例如 `profiles:read`、`profiles:write`、`runtime:control`。
- `event`: 服务端通过 WebSocket 推送的运行时、代理和审计事件，带单调递增标识。
- `idempotency key`: 客户端为启动/停止等写操作提供的唯一键，用于安全重试。
- Interface: `/api/v1/*` HTTP JSON；`/api/v1/events` WebSocket；`/healthz`、`/readyz`；服务端同源静态 Web UI；桌面 Tauri IPC 镜像相同命令语义。

## Requirements

### REQ-API-001

- The API MUST expose versioned JSON endpoints under `/api/v1`, and breaking changes MUST use a new version path with a documented deprecation window.
- Inputs: HTTP 请求、路径、请求头、JSON 请求体。
- Outputs: JSON 响应；未知版本返回稳定错误；同一版本内的字段只做向后兼容扩展。

### REQ-API-002

- The API MUST publish a machine-readable OpenAPI document that covers all REST endpoints, request/response schemas, auth schemes, error codes, and pagination parameters.
- Inputs: 构建时的路由与 schema 定义。
- Outputs: 可被客户端生成器读取的 OpenAPI 文件；运行时 `/api/v1/openapi.json` 可获取同一契约。

### REQ-API-003

- The server host MUST require authentication for all non-health endpoints, using a login-created session or a scoped API token.
- Inputs: 登录凭据、Cookie、Bearer Token、请求路径。
- Outputs: 认证成功时返回资源；缺失或无效认证返回 401；权限不足返回 403；健康端点只暴露最小信息。

### REQ-API-004

- The desktop host MUST operate in local single-user mode without a login, and MUST expose its optional automation HTTP API only on loopback with a generated token and explicit user enablement.
- Inputs: 桌面设置、Token 创建/撤销、回环请求。
- Outputs: 桌面 UI 无需登录；自动化 API 默认关闭；启用后非回环请求被拒绝；Token 可撤销。

### REQ-API-005

- The system MUST hash API tokens at rest, support revocation and last-used tracking, and MUST enforce token scopes on every operation.
- Inputs: Token 创建、Token 值、请求操作、Token 记录。
- Outputs: 明文 Token 只在创建时返回一次；数据库只有哈希；撤销后立即失效；越权操作返回 403。

### REQ-API-006

- The system MUST protect cookie-authenticated writes with CSRF defenses, including `SameSite=Strict`, origin validation, and a per-session CSRF token or equivalent custom header.
- Inputs: 会话 Cookie、`Origin`/`Referer`、CSRF Token、写请求。
- Outputs: 合法同源请求通过；跨源、缺失或错误 CSRF 的写请求被拒绝；Bearer Token 请求不依赖 Cookie CSRF。

### REQ-API-007

- The system MUST rate-limit login attempts and MUST apply temporary lockout after repeated failures without revealing whether a username exists.
- Inputs: 登录请求、来源地址、失败次数、时间窗口。
- Outputs: 正常尝试通过；超限返回 429 和重试时间；错误消息不区分“用户不存在”和“密码错误”。

### REQ-API-008

- The system MUST return a unified error envelope containing a stable code, human-readable message, request identifier, and optional non-sensitive details.
- Inputs: 校验失败、认证失败、资源冲突、运行时错误、内部错误。
- Outputs: 一致 JSON 结构；HTTP 状态与错误码匹配；不泄露密码、Token、Cookie、完整代理 URL 或堆栈。

### REQ-API-009

- The system MUST support cursor-based pagination, bounded page sizes, stable sorting, and filters for profile and audit listings.
- Inputs: 游标、页大小、筛选、排序。
- Outputs: 结果页与下一页游标；超出上限的页大小被拒绝或截断并明确说明；重复请求不产生重复或遗漏。

### REQ-API-010

- The system MUST make start, stop, and other resource-mutating POST requests idempotent when an `Idempotency-Key` is supplied, returning the original result for duplicate keys within a bounded retention window.
- Inputs: `Idempotency-Key`、请求体、档案状态。
- Outputs: 首次请求执行；重复键返回相同状态与结果；结果包含运行时标识、状态、CDP 地址（如启用）和本机代理端口；键冲突且请求体不同返回 409；过期键可重新使用。

### REQ-API-011

- The system MUST authenticate WebSocket connections without placing long-lived tokens in URLs, and MUST support event replay from a client-provided event identifier.
- Inputs: 会话 Cookie 或一次性票据、`after` 事件标识、心跳。
- Outputs: 认证通过后建立连接；断线重连可从最后事件继续；无效认证被拒绝；心跳用于检测死连接。

### REQ-API-012

- The system MUST provide liveness and readiness endpoints with distinct semantics and MUST NOT require authentication for their minimal response.
- Inputs: 服务进程、数据库、内核目录、协议引擎目录、显示环境。
- Outputs: `/healthz` 仅表示进程存活；`/readyz` 仅在数据库、内核、协议引擎和显示环境可用时成功；响应不包含敏感配置。

### REQ-API-013

- The system MUST enforce request body size limits, JSON schema validation, content-type checks, and timeouts for all endpoints.
- Inputs: 请求体、Content-Type、路由、超时。
- Outputs: 超限或非法请求返回 4xx；服务端不因超大或畸形请求耗尽内存；导入接口有独立且更严格的上限。

### REQ-API-014

- The system MUST record audit events for authentication, profile mutations, runtime control, proxy changes, token changes, kernel changes, and viewer access.
- Inputs: 已认证操作者、请求标识、目标、结果。
- Outputs: 审计事件包含操作者、动作、目标、结果、时间和请求标识；敏感值脱敏；审计可通过 API 分页读取。

### REQ-API-015

- The system MUST serve the Web UI from the same origin as the API in server mode and MUST set security headers including a restrictive Content-Security-Policy, `X-Content-Type-Options`, `Referrer-Policy`, and frame-ancestors restrictions.
- Inputs: 静态资源、路由、反向代理配置。
- Outputs: 同源加载成功；CSP 阻止未授权脚本与内联执行；点击劫持与 MIME 嗅探被阻止；API 与 UI 不跨源混用。

### REQ-API-016

- The system MUST NOT expose CDP or noVNC through the control API by default, and when viewer access is enabled it MUST return a short-lived one-time URL rather than a reusable credential.
- Inputs: 查看器请求、会话、Token scope、容器网络。
- Outputs: 默认无查看器端点；启用后返回一次性 URL；URL 过期或使用一次后失效；CDP 仍只绑定回环。

### REQ-API-017

- The system MUST expose proxy configuration CRUD, single share-link import, and proxy check endpoints, MUST validate inputs against the protocol engine capability manifest, and MUST return only redacted proxy views.
- Inputs: 代理配置、`vless://`/`vmess://`/`ss://`/`trojan://`/`hysteria2://` 链接、Token scope、引擎能力清单。
- Outputs: 脱敏代理记录、导入草稿或字段级错误；任何响应不得包含 UUID、密码、私钥、完整分享链接或节点 URL 中的敏感参数；检查通过协议引擎执行。

## Verification

### VER-API-001

- Method: 请求 `/api/v1` 与未知版本路径，检查响应和弃用头。
- covers: `REQ-API-001`
- Pass condition: 已知版本成功；未知版本返回稳定错误；弃用端点带弃用信息。

### VER-API-002

- Method: 读取 OpenAPI 文件与运行时端点，使用生成器生成客户端。
- covers: `REQ-API-002`
- Pass condition: 两者内容一致；所有路由与错误码可解析；生成客户端可编译。

### VER-API-003

- Method: 未认证、无效 Token、有效会话、越权 Token 分别访问读写端点。
- covers: `REQ-API-003`
- Pass condition: 未认证 401；无效 Token 401；有效会话通过；越权 403；健康端点无需认证。

### VER-API-004

- Method: 在桌面模式启用自动化 API，从回环与非回环地址请求，并撤销 Token。
- covers: `REQ-API-004`
- Pass condition: 默认关闭；启用后回环成功；非回环失败；撤销后请求失败；UI 无需登录。

### VER-API-005

- Method: 创建 Token，检查数据库与创建响应；使用不同 scope 调用；撤销后重试。
- covers: `REQ-API-005`
- Pass condition: 明文只出现一次；数据库只存哈希；scope 生效；撤销立即生效；last_used 更新。

### VER-API-006

- Method: 使用合法同源、跨源、缺失 CSRF、错误 CSRF 和 Bearer Token 发起写请求。
- covers: `REQ-API-006`
- Pass condition: 合法同源通过；跨源与错误 CSRF 被拒绝；Bearer Token 请求通过且不依赖 Cookie。

### VER-API-007

- Method: 连续提交错误密码并观察响应时间与锁定行为。
- covers: `REQ-API-007`
- Pass condition: 超限返回 429；锁定时间可配置；响应不泄露用户是否存在；锁定结束后可重试。

### VER-API-008

- Method: 构造校验、认证、冲突、运行时和内部错误，扫描响应。
- covers: `REQ-API-008`
- Pass condition: 错误结构一致；状态码与错误码匹配；请求标识存在；无敏感值或堆栈。

### VER-API-009

- Method: 创建 100 个档案与审计事件，使用游标翻页并改变筛选。
- covers: `REQ-API-009`
- Pass condition: 无重复或遗漏；页大小上限生效；排序稳定；非法游标返回明确错误。

### VER-API-010

- Method: 使用相同和不同幂等键重复启动/停止请求。
- covers: `REQ-API-010`
- Pass condition: 相同键返回原结果；不同请求体返回 409；过期后可重新执行；无重复运行时。

### VER-API-011

- Method: 建立 WebSocket，断开后带 `after` 重连，使用 URL Token 尝试连接。
- covers: `REQ-API-011`
- Pass condition: 正常认证成功；重连补齐事件；URL Token 被拒绝；心跳检测死连接。

### VER-API-012

- Method: 停止数据库、内核或显示环境，调用健康与就绪端点。
- covers: `REQ-API-012`
- Pass condition: 存活与就绪语义不同；就绪在依赖缺失时失败；响应无敏感配置。

### VER-API-013

- Method: 发送超大 JSON、错误 Content-Type、畸形 JSON、慢请求和超大导入文件。
- covers: `REQ-API-013`
- Pass condition: 请求被 4xx 拒绝；内存不持续增长；超时连接被关闭；导入上限独立生效。

### VER-API-014

- Method: 执行登录、档案增删、启动停止、代理变更、Token 变更和查看器访问。
- covers: `REQ-API-014`
- Pass condition: 每类操作产生审计事件；字段完整；敏感值脱敏；可分页读取。

### VER-API-015

- Method: 检查响应头，尝试内联脚本、跨源嵌入和 MIME 嗅探。
- covers: `REQ-API-015`
- Pass condition: CSP、`X-Content-Type-Options`、`Referrer-Policy` 和 frame-ancestors 生效；恶意页面无法嵌入或执行。

### VER-API-016

- Method: 默认请求查看器端点，启用后使用一次性 URL 两次，并尝试从容器外连接 CDP。
- covers: `REQ-API-016`
- Pass condition: 默认不可用；一次性 URL 第二次失败；CDP 从容器外不可达；API 不返回可复用凭据。

### VER-API-017

- Method: 创建、更新、删除 HTTP/SOCKS/VLESS/VMess/Shadowsocks/Trojan 代理，导入合法与非法分享链接，执行检查并扫描响应。
- covers: `REQ-API-017`
- Pass condition: 合法配置成功；非法协议、传输和参数被拒绝；响应只含脱敏字段；检查结果来自协议引擎；越权 Token 返回 403。

## Related ADRs

- [ADR-0001：共享 Rust 核心与双宿主](../../adr/0001-shared-core-two-hosts.md)
- [ADR-0003：SQLite 存储与字段级密钥保护](../../adr/0003-storage-and-secret-protection.md)
- [ADR-0007：协议引擎承载正向代理](../../adr/0007-protocol-engine-forward-proxy.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
