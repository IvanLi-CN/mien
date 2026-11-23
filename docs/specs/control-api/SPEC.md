# 控制面 API 与鉴权

> 本文件是控制面主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: 桌面与服务端共享档案、指纹、代理和运行时能力，但宿主认证模型不同。服务端需要登录、会话和自动化 Token；桌面默认本地单用户，可通过可选回环 API 暴露自动化入口。
- In scope: REST 与 WebSocket 契约、鉴权、授权范围、CSRF、错误结构、分页、幂等、事件、健康检查、请求限制、审计、静态 UI 安全头。
- Out of scope: 指纹语义（由 `fingerprint-engine` 负责）、运行时进程细节（由 `browser-runtime` 负责）、代理协议（由 `proxy-gateway` 负责）。

## Terms and Interfaces

- `session`: 服务端登录后创建的浏览器会话，使用 `HttpOnly`、`SameSite=Strict` Cookie。
- `API token`: 自动化客户端使用的 Bearer Token，只保存哈希，带授权范围。
- `scope`: Token 允许执行的操作集合，使用点号形式，例如 `profiles.read`、`profiles.write`、`runtime.control`、`tokens.manage`、`events.read`、`viewer.access`。OpenAPI 保持现有 HTTP Bearer Token scheme，以 `x-required-scope` 声明每个操作所需 scope；CI 将该值与 `TokenScope` 枚举逐项比对，不引入 OAuth2 token exchange。
- `public allowlist`: 无需认证即可访问的端点集合，固定为 `GET /healthz`、`GET /readyz`、`GET /api/v1` 版本发现、`POST /api/v1/auth/login` 和 `GET /api/v1/openapi.json`；其他端点一律需要认证。
- `event`: 服务端通过 WebSocket 推送的运行时、代理和审计事件，`sequence` 为单调递增整数；断线重连使用 `after=<sequence>` 补齐。
- `event ticket`: 单次使用的短 TTL WebSocket 票据；OpenAPI 声明 `ticketAuth`（`type: apiKey`、`in: query`、`name: ticket`），不得用匿名 `security: []` 代替。
- `one-time secret`: `TokenCreated.token` 与 `EventTicket.ticket` 只在创建响应中出现一次，是仅响应字段，不能作为请求字段，也不能通过幂等重放再次返回。
- `idempotency key`: 客户端为启动/停止、Token 创建等写操作提供的唯一键，用于安全重试；结果缓存与冲突规则见 REQ-API-010。
- `profile log tail`: 运行时有界日志尾部，由 `GET /api/v1/profiles/{profile_id}/logs?limit=` 返回；来源为运行时日志环或轮转文件，响应已脱敏。
- `export/import`: 导出请求体 `ProfileExportRequest` = `{format: plain|encrypted, passphrase?}`，导入请求体 `ProfileImportInput` = `{format, payload, passphrase?, conflict_strategy?: new_id|abort}`；`conflict_strategy` 默认 `new_id`，`passphrase` 为 write-only；载荷语义由 `profile-management` spec 与 `profile-export.schema.json` 定义。
- Interface: `/api/v1/*` HTTP JSON；`/api/v1/events` WebSocket（`ticketAuth`）；`/healthz`、`/readyz`；`POST /api/v1/profiles/{profile_id}/export`、`POST /api/v1/profiles/import/validate`、`POST /api/v1/profiles/import`、`GET /api/v1/profiles/{profile_id}/logs`；服务端同源静态 Web UI；桌面 Tauri IPC 镜像相同命令语义。

## Requirements

### REQ-API-001

- The API MUST expose versioned JSON endpoints under `/api/v1`, MUST provide `GET /api/v1` for version and deprecation-window discovery, and breaking changes MUST use a new version path with a documented deprecation window.
- Inputs: HTTP 请求、路径、请求头、JSON 请求体。
- Outputs: JSON 响应；未知版本返回稳定错误；同一版本内的字段只做向后兼容扩展。

### REQ-API-002

- The API MUST publish a machine-readable OpenAPI document that covers all REST endpoints, request/response schemas, auth schemes (`cookieAuth`、HTTP `bearerAuth`、`ticketAuth`), error codes, and pagination parameters, and CI MUST run a generation-diff check so the committed document cannot drift from route/schema definitions. Each protected operation declares `x-required-scope`; CI MUST validate it against `TokenScope`.
- Inputs: 构建时的路由与 schema 定义。
- Outputs: 可被客户端生成器读取的 OpenAPI 文件；运行时 `/api/v1/openapi.json` 可获取同一契约；重新生成产生非空 diff 时契约作业失败。

### REQ-API-003

- The server host MUST require authentication for every endpoint outside the `public allowlist`, using a login-created session or a scoped API token, and MUST declare a deterministic status-code contract: every protected operation declares 401 and 403, every operation addressing an existing resource declares 404, and existing 409/422 semantics are preserved.
- Precedence: 认证先于授权与资源存在性判定；无会话或无效凭据一律返回 401，不得因目标不存在而改成 404；已认证但缺少所需 scope 时返回 403，且在检查资源存在性之前判定，避免用 403/404 差异枚举资源；已授权但目标不存在时返回 404。软删除档案对启动或会改变档案的写操作返回 409 `PROFILE_DELETED`；restore、purge、读取与删除重试仍可用；已 purge 的档案与从未存在的 ID 返回 404 `PROFILE_NOT_FOUND`。
- Inputs: 登录凭据、Cookie、Bearer Token、`ticket`、请求路径。
- Outputs: 公开端点无认证可用；受保护端点认证成功时返回资源；缺失或无效认证返回 401；scope 不足返回 403；资源不存在返回 404；健康与版本发现端点只暴露最小信息。

### REQ-API-004

- The desktop host MUST operate in local single-user mode without a login, and MUST expose its optional automation HTTP API only on loopback with a generated token and explicit user enablement.
- Inputs: 桌面设置、Token 创建/撤销、回环请求。
- Outputs: 桌面 UI 无需登录；自动化 API 默认关闭；启用后非回环请求被拒绝；Token 可撤销。

### REQ-API-005

- The system MUST hash API tokens at rest, support revocation and last-used tracking, and MUST enforce token scopes on every operation. Scopes are the `TokenScope` enum; HTTP Bearer authenticates the token and each operation declares its required scope with `x-required-scope`, which CI validates against that enum. No OAuth2 token exchange endpoint is introduced; token creation MUST NOT grant scopes the creator does not hold.
- One-time secrets: `TokenCreated.token` and `EventTicket.ticket` are response-only fields, MUST NOT be declared write-only request inputs, and MUST never be logged, echoed, or replayed by an idempotent retry.
- Inputs: Token 创建、Token 值、请求操作、Token 记录、scope 声明。
- Outputs: 明文 Token 只在创建响应中出现一次；数据库只有哈希；撤销后立即失效；越权操作返回 403；请求体中出现一次性秘密字段被视为无效请求。

### REQ-API-006

- The system MUST protect cookie-authenticated writes and WebSocket upgrades with CSRF/origin defenses, including `SameSite=Strict`, origin validation on every cookie-authenticated upgrade, and a per-session CSRF token or equivalent custom header.
- Inputs: 会话 Cookie、`Origin`/`Referer`、CSRF Token、写请求。
- Outputs: 合法同源请求通过；跨源、缺失或错误 CSRF 的写请求被拒绝；Bearer Token 请求不依赖 Cookie CSRF。

### REQ-API-007

- The system MUST rate-limit login attempts and MUST apply temporary lockout after repeated failures without revealing whether a username exists. Lockout responses MUST include `Retry-After` seconds.
- Inputs: 登录请求、来源地址、失败次数、时间窗口。
- Outputs: 正常尝试通过；超限返回 429 和重试时间；错误消息不区分“用户不存在”和“密码错误”。

### REQ-API-008

- The system MUST return a unified error envelope containing a stable code, human-readable message, request identifier, and optional non-sensitive details. The error `code` MUST be a member of the `ErrorCode` enum generated from the error-code catalog; event `error_code` fields and test fixtures MUST use the same set, and CI MUST compare catalog, OpenAPI enum, event schema and HTTP mappings in both directions.
- Inputs: 校验失败、认证失败、资源冲突、运行时错误、内部错误、错误码目录。
- Outputs: 一致 JSON 结构；HTTP 状态与错误码匹配；目录与契约任一方向不一致（新增、删除或状态映射漂移）时 CI 失败；不泄露密码、Token、Cookie、完整代理 URL 或堆栈。

### REQ-API-009

- The system MUST support cursor-based pagination, bounded page sizes, stable sorting, and filters for profile and audit listings, including archived profile status and audit `event_type`/`profile_id`/`action`/`from`/`to`. `AuditEvent` MUST carry `event_type` and `profile_id`; `action` remains for compatibility and is not a substitute for `event_type`.
- Inputs: 游标、页大小、筛选、排序。
- Outputs: 结果页与下一页游标；未知筛选字段或非法枚举返回 422；超出上限的页大小被拒绝或截断并明确说明；重复请求不产生重复或遗漏；UI 活动页与审计端点使用同一筛选字段名。

### REQ-API-010

- The system MUST make start, stop, and other non-secret resource-mutating POST requests idempotent when an `Idempotency-Key` is supplied, returning the original result for duplicate keys within a bounded retention window. `tokensCreate` MUST accept `Idempotency-Key`, but because the plaintext secret is one-time it MUST NOT replay a cached response: a repeated key on `tokensCreate` returns 409 `IDEMPOTENCY_CONFLICT` even when the body is identical.
- Inputs: `Idempotency-Key`、请求体、档案状态、Token 创建请求。
- Outputs: 首次请求执行；重复键返回相同状态与结果；结果包含运行时标识、状态、CDP 地址（如启用）和本机代理端口；键冲突且请求体不同返回 409；`tokensCreate` 的重复键一律 409 且不返回明文 Token；过期键可重新使用。

### REQ-API-011

- The system MUST authenticate WebSocket connections with the `ticketAuth` scheme (one-time ticket from `POST /api/v1/events/ticket`) or a session cookie, MUST reject long-lived tokens in URLs, and MUST validate `Origin` on every upgrade, including ticket-authenticated ones. The OpenAPI operation MUST declare `security: [{ticketAuth: []}, {cookieAuth: []}]` and MUST NOT use an anonymous `security: []`.
- Replay: the cursor is the integer `event.sequence`; `after=<sequence>` replays retained events with a greater sequence. A non-integer or non-positive cursor is rejected with 422, and the server MUST NOT silently skip retained events. The replay window bound is recorded as an implementation fact.
- Inputs: 会话 Cookie 或一次性票据、`ticket` 查询参数、整数 `after`、`Origin`、心跳。
- Outputs: 认证通过后建立连接；断线重连按 `sequence > after` 补齐；缺失、过期或已使用的票据返回 401 `EVENT_TICKET_INVALID`；跨源升级返回 403 `AUTH_ORIGIN_INVALID`；URL 长效 Token 被拒绝；心跳用于检测死连接。

### REQ-API-012

- The system MUST provide liveness and readiness endpoints with distinct semantics and MUST NOT require authentication for their minimal response. Readiness MUST include the display environment only when the configured runtime modes require it; pure headless deployments must be able to become ready without Xvfb.
- Inputs: 服务进程、数据库、内核目录、共享 Xray 引擎、显示环境。
- Outputs: `/healthz` 仅表示进程存活；`/readyz` 在数据库、内核、共享 Xray 引擎以及当前运行模式所需显示环境可用时成功；纯 headless 部署不要求 Xvfb；响应不包含敏感配置。

### REQ-API-013

- The system MUST enforce request body size limits, JSON schema validation, content-type checks, and timeouts for all endpoints.
- Inputs: 请求体、Content-Type、路由、超时。
- Outputs: 超限或非法请求返回 4xx；服务端不因超大或畸形请求耗尽内存；导入接口有独立且更严格的上限。

### REQ-API-014

- The system MUST record audit events for authentication, profile mutations, runtime control, proxy changes, token changes, kernel changes, and viewer access. Every audit record MUST include the originating `request_id`, actor, target, typed `event_type`, compatibility `action`, result, and time.
- Inputs: 已认证操作者、请求标识、目标、结果。
- Outputs: 审计事件包含操作者、`event_type`、`action`、目标、结果、时间和请求标识；敏感值脱敏；审计可通过 API 分页读取并按 `event_type`/`profile_id`/`action`/时间筛选。

### REQ-API-015

- The system MUST serve the Web UI from the same origin as the API in server mode and MUST set security headers including a restrictive Content-Security-Policy, `X-Content-Type-Options`, `Referrer-Policy`, and frame-ancestors restrictions.
- Inputs: 静态资源、路由、反向代理配置。
- Outputs: 同源加载成功；CSP 阻止未授权脚本与内联执行；点击劫持与 MIME 嗅探被阻止；API 与 UI 不跨源混用。

### REQ-API-016

- The system MUST NOT expose CDP or noVNC through the control API by default. It MUST expose `POST /api/v1/profiles/{profile_id}/viewer`, protected by the `viewer.access` scope, which returns `VIEWER_NOT_AVAILABLE` by default and a short-lived one-time URL when viewer access is enabled.
- Inputs: 查看器请求、会话、Token scope、容器网络。
- Outputs: 默认无查看器端点；启用后返回一次性 URL；URL 过期或使用一次后失效；CDP 仍只绑定回环。

### REQ-API-017

- The system MUST expose proxy configuration CRUD, single share-link import, and proxy check endpoints, MUST validate inputs against the protocol engine capability manifest, and MUST return only redacted proxy views.
- Inputs: 代理配置、`vless://`/`vmess://`/`ss://`/`trojan://`/`hysteria2://` 链接、Token scope、引擎能力清单。
- Outputs: 脱敏代理记录、导入草稿或字段级错误；任何响应不得包含 UUID、密码、私钥、完整分享链接或节点 URL 中的敏感参数；检查通过协议引擎执行，网络失败以 HTTP 200 的 `ProxyCheck.error_code` 返回并同时写入事件，不使用未声明的 502；检查结果仅作警告，不阻断保存或启动。

### REQ-API-018

- The system MUST require a first-login password change for the generated administrator password and MUST expose `POST /api/v1/auth/password` for the authenticated session.
- Inputs: 当前密码、新密码、会话、CSRF Token。
- Outputs: 首次登录响应标记 `must_change_password`；未改密前除改密和登出外的写操作返回 `AUTH_PASSWORD_CHANGE_REQUIRED`；改密成功后其他会话失效。

### REQ-API-019

- The system MUST expose `GET /api/v1/profiles/{profile_id}/logs?limit=` for a bounded, redacted runtime log tail, backed by the runtime log ring or rotated log files.
- Inputs: 档案 ID、`limit` 查询参数、会话或 `profiles.read` scope、运行时日志环/轮转文件。
- Outputs: 200 响应包含不超过 `limit` 的有界日志尾部及截断标记（字段由 OpenAPI 定义）；运行时存在但尚无日志时返回 200 空 items；`limit` 非法或超出上限返回 422；未认证 401；缺少 scope 403；档案不存在 404 `PROFILE_NOT_FOUND`；既无运行时也无保留日志时 404 `RUNTIME_NOT_FOUND`；响应不含凭据、Cookie、授权头、UUID、完整 URL 或完整代理配置；桌面宿主提供等价 IPC/本地日志回退。
- Semantics: 日志写入、轮转与脱敏规则由 `browser-runtime` REQ-RUNTIME-015 定义，本要求固定 HTTP 边界与授权。

### REQ-API-020

- The system MUST expose profile export and import as `POST /api/v1/profiles/{profile_id}/export`, `POST /api/v1/profiles/import/validate` and `POST /api/v1/profiles/import`, matching the request/response shapes and payload semantics defined by `profile-management`.
- Inputs: `ProfileExportRequest`、`ProfileImportInput`、CSRF Token 或 Bearer Token、`profiles.read`/`profiles.write` scope。
- Outputs: 导出返回 `ProfileExportEnvelope`；`passphrase` 为 write-only，加密格式必填、明文格式禁止，且不得出现在 URL、查询串、日志、审计或错误消息；明文 `payload` 遵循 `profile-export.schema.json`；加密 `payload` 是不透明、带版本号的容器，使用 Argon2id 派生密钥与认证加密，envelope 版本参与 AAD。
- Import: `import/validate` 是 dry-run，不写行、不建目录，返回逐项草稿/errors/冲突；`import` 返回 `ProfileImportResult`，档案冲突以导出项 `source_profile_id` 匹配 `profiles.import_source_id` 或现有 `profiles.id`，分组/标签冲突按名称匹配；`conflict_strategy=new_id` 分配新档案 ID 并保留 `import_source_id`，`abort` 在写入前返回 `PROFILE_IMPORT_CONFLICT`。
- Redaction: 导出中的代理只含 `credential_fields` 名称与非秘密 `options`，`credentials_removed=true`；秘密键被拒绝；导出不包含顶层 `runtime.window`，窗口尺寸只在指纹规格内。

### REQ-API-021

- The system MUST expose profile and runtime payloads that make lifecycle, run mode, direct mode and isolation state explicit: profile `status` is `active|archived|deleted` with `deleted_at` and `deletion_error`; runtime `state` is `starting|running|stopping|stopped|crashed|failed`, `mode` is `headful|headless|pseudo_headful`; both payloads carry `isolation` (`enforced|unsandboxed|unknown`), `direct` and `reduced_assurance`, and `pseudo_headful` also carries `ua_scope=chrome`.
- Inputs: 档案与运行时记录、启动事务、沙箱探测结果。
- Outputs: `direct=true` 当且仅当档案未配置代理并进入直连模式；`reduced_assurance=true` 当且仅当 `mode` 为 `headless`/`pseudo_headful`、`direct=true` 或 `isolation != enforced`；已停止档案仍返回最后一次 `isolation` 与相应降低保证；`deleted` 档案启动返回 409 `PROFILE_DELETED`；purge 失败返回 `deletion_error` 供重试；API、IPC 与数据库中不存在 `block_start_on_check_failure` 字段，代理检查结果只作警告，不阻断启动。
- Semantics: 运行时状态机、沙箱校验与隔离推导由 `browser-runtime` 负责，档案生命周期由 `profile-management` 负责，本要求固定对外字段与缺失值的表达。

## Verification

### VER-API-001

- Method: 请求 `/api/v1` 与未知版本路径，检查响应和弃用头。
- covers: `REQ-API-001`
- Pass condition: 已知版本成功；未知版本返回稳定错误；弃用端点带弃用信息。

### VER-API-002

- Method: 读取 OpenAPI 文件与运行时端点，重新从路由/schema 生成文档并比对，使用生成器生成客户端。
- covers: `REQ-API-002`
- Pass condition: 运行时与仓库文档一致；重新生成无 diff；所有路由、auth scheme（含 Bearer 与 `ticketAuth`）以及 `x-required-scope`与错误码可解析；生成客户端可编译。

### VER-API-003

- Method: 逐个请求公开与受保护端点；对受保护端点分别使用无凭据、无效 Token、有效会话、缺 scope Token，并对不存在与软删除的档案 ID 发起请求。
- covers: `REQ-API-003`
- Pass condition: 仅 `/healthz`、`/readyz`、`GET /api/v1`、`POST /api/v1/auth/login`、`GET /api/v1/openapi.json` 无需认证；无凭据或无效凭据一律 401；缺 scope 返回 403 且不泄露资源是否存在；已授权但资源不存在返回 404；软删除档案的启动/修改返回 409 `PROFILE_DELETED`，restore/purge/读取仍可用；OpenAPI 为每个受保护操作声明 401/403，为按 ID 寻址的操作声明 404。

### VER-API-004

- Method: 在桌面模式启用自动化 API，从回环与非回环地址请求，并撤销 Token。
- covers: `REQ-API-004`
- Pass condition: 默认关闭；启用后回环成功；非回环失败；撤销后请求失败；UI 无需登录。

### VER-API-005

- Method: 创建 Token，检查数据库与创建响应；使用不同 scope 调用，对照 OpenAPI 的 Bearer 与 `x-required-scope` 声明；尝试在请求体携带 `token`/`ticket`；撤销后重试。
- covers: `REQ-API-005`
- Pass condition: 明文只在 201 响应出现一次；数据库只存展示前缀与 HMAC 哈希，前缀不能用于认证；OpenAPI 使用 HTTP Bearer 且每个受保护操作带 `x-required-scope`，CI 校验其属于 `TokenScope`；越权 403；请求体出现一次性秘密字段被拒绝；撤销立即生效；last_used 更新。

### VER-API-006

- Method: 使用合法同源、跨源、缺失 CSRF、错误 CSRF 和 Bearer Token 发起写请求。
- covers: `REQ-API-006`
- Pass condition: 合法同源通过；跨源与错误 CSRF 被拒绝；Bearer Token 请求通过且不依赖 Cookie。

### VER-API-007

- Method: 连续提交错误密码并观察响应时间与锁定行为。
- covers: `REQ-API-007`
- Pass condition: 超限返回 429；锁定时间可配置；响应不泄露用户是否存在；锁定结束后可重试。

### VER-API-008

- Method: 构造校验、认证、冲突、运行时和内部错误，扫描响应；运行错误码目录与 OpenAPI/事件 schema 的 enum 双向比对，并注入一个仅存在于目录的错误码。
- covers: `REQ-API-008`
- Pass condition: 错误结构一致；状态码与错误码映射匹配；请求标识存在；无敏感值或堆栈；目录与契约完全一致，注入单向漂移时 CI 失败。

### VER-API-009

- Method: 创建 100 个档案与审计事件，使用游标翻页并按 `event_type`、`profile_id`、`action`、`from`/`to` 组合筛选，再注入非法枚举。
- covers: `REQ-API-009`
- Pass condition: 无重复或遗漏；页大小上限生效；排序稳定；非法游标/枚举返回明确错误；`AuditEvent` 同时包含 `event_type` 与 `profile_id`，`action` 保留兼容；UI 筛选字段与 API 一致。

### VER-API-010

- Method: 使用相同和不同幂等键重复启动/停止请求；对 `tokensCreate` 使用相同键重复请求（相同与不同请求体）。
- covers: `REQ-API-010`
- Pass condition: 开始/停止相同键返回原结果；不同请求体返回 409；过期后可重新执行；无重复运行时；`tokensCreate` 重复键一律 409 `IDEMPOTENCY_CONFLICT`，不返回缓存的 201 响应或明文 Token。

### VER-API-011

- Method: 检查 OpenAPI 的 `/api/v1/events` security 声明；申请一次性票据建立 WebSocket，断开后带整数 `after=<sequence>` 重连，并测试非整数/非正游标、URL 长效 Token、缺票据与跨源 Origin。
- covers: `REQ-API-002`, `REQ-API-011`
- Pass condition: OpenAPI 声明 `ticketAuth`（apiKey in query）与 `cookieAuth`，不存在匿名 `security: []`；票据只能使用一次，缺失/过期/已用返回 401 `EVENT_TICKET_INVALID`；重连按 `sequence > after` 补齐且不跳过保留事件；非整数或非正 `after` 返回 422；URL 长效 Token 被拒绝；跨源升级返回 403 `AUTH_ORIGIN_INVALID`；心跳检测死连接。

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
- Pass condition: 每类操作产生审计事件；`request_id`、actor、target 完整；敏感值脱敏；可按 profile/type/时间过滤并分页读取。

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

### VER-API-018

- Method: 使用生成的初始密码登录，尝试写操作，改密后重新登录，并尝试复用旧会话。
- covers: `REQ-API-018`
- Pass condition: 首次登录返回 `must_change_password=true`；未改密写操作返回 403 `AUTH_PASSWORD_CHANGE_REQUIRED`；改密成功后旧会话失效，新密码可登录。

### VER-API-019

- Method: 在运行与停止的档案上调用 `GET /api/v1/profiles/{id}/logs?limit=`，分别使用无认证、缺 scope、不存在 ID、无保留日志、非法与超限 `limit`，并扫描响应与桌面 IPC 等价输出。
- covers: `REQ-API-019`, `REQ-RUNTIME-015`
- Pass condition: 200 返回不超过 `limit` 的有界尾部与截断标记；非法/超限 `limit` 返回 422；未认证 401；缺 scope 403；档案不存在 404 `PROFILE_NOT_FOUND`；无运行时且无保留日志 404 `RUNTIME_NOT_FOUND`；响应与 IPC 输出均无凭据、Cookie、授权头、UUID 或完整 URL。

### VER-API-020

- Method: 分别以 `POST /api/v1/profiles/{id}/export` 导出 plain/encrypted，调用 `import/validate` 与 `import` 使用 `new_id`/`abort`，并在日志、审计、URL 中搜索口令。
- covers: `REQ-API-020`
- Pass condition: plain 载荷通过 `profile-export.schema.json` 且无凭据/Token/Cookie/顶层 `window`；encrypted 载荷无口令不可读、正确口令可还原、错误口令返回字段级错误；口令不出现在 URL、日志、审计与错误；validate 不写行/不建目录；`abort` 在写入前返回 `PROFILE_IMPORT_CONFLICT`；代理仅含 `credential_fields` 与非秘密 `options`，`credentials_removed=true`。

### VER-API-021

- Method: 读取档案与运行时响应（覆盖六态与停止后重读）、软删除档案启动、模拟 purge 失败，并搜索 API/IPC schema 与数据库中是否存在 `block_start_on_check_failure`。
- covers: `REQ-API-021`
- Pass condition: 运行时 `state` 为六态之一且 `mode` 为三档之一；档案 `status` 为 `active|archived|deleted` 并带 `deleted_at`/`deletion_error`；两者均含 `isolation` 与 `direct`，停止档案返回最后一次 `isolation` 与正确推导的 `reduced_assurance`；直连档案 `direct=true` 且 `reduced_assurance=true`；`pseudo_headful` 返回 `ua_scope=chrome`；`deleted` 启动返回 409 `PROFILE_DELETED`；purge 失败返回 `deletion_error` 可重试；`block_start_on_check_failure` 不存在，代理检查不阻断启动。

## Related ADRs

- [ADR-0001：共享 Rust 核心与双宿主](../../adr/0001-shared-core-two-hosts.md)
- [ADR-0003：SQLite 存储与字段级密钥保护](../../adr/0003-storage-and-secret-protection.md)
- [ADR-0009：单实例 Xray 动态协议引擎与连接复用](../../adr/0009-shared-xray-engine.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
