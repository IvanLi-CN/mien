# 控制面 API 主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题在开发启动前建立，API 版本从 `/api/v1` 开始。
- 桌面与服务端共享命令语义，但认证模型不同；桌面默认本地单用户，服务端使用会话与 Token。
- 公开白名单固定为 `/healthz`、`/readyz`、`GET /api/v1` 版本发现、`POST /api/v1/auth/login` 与 `GET /api/v1/openapi.json`；其余端点必须认证。
- 事件 `sequence` 为整数，重连使用 `after=<sequence>`；WebSocket 升级使用一次性 `ticketAuth`（apiKey in query）或会话 Cookie，不再使用匿名 `security: []` 或 URL 长效 Token。
- 保留 HTTP Bearer Token 认证；scope 由每个操作的 `x-required-scope` 表达并由 CI 对照 `TokenScope` 校验，不增加 OAuth2 exchange endpoint；`tokensCreate` 接受 `Idempotency-Key`，但重复键返回 409 且从不重放明文 Token。
- 控制面增加代理配置 CRUD、分享链接导入与检查端点；响应只返回脱敏视图，凭据不可读回，检查网络失败放在 200 响应的 `ProxyCheck.error_code`。
- 增加档案导出/导入端点（`POST .../export`、`import/validate`、`import`）与有界日志尾部端点（`GET /api/v1/profiles/{id}/logs?limit=`）；口令只允许出现在请求体，日志尾部必须脱敏。
- 错误响应 `code` 使用 `ErrorCode` 枚举；目录、OpenAPI 与事件 schema 的双向漂移由 CI 阻断。

## Replacements / Background

- 选择 REST + WebSocket 而不是 GraphQL，是为了降低服务端复杂度、便于生成客户端和调试。
- 选择会话 Cookie 与 Bearer Token 并存，是为了同时覆盖浏览器 UI 和自动化客户端。
- WebSocket 不使用 URL 长效 Token，避免令牌进入日志与历史记录；使用会话 Cookie 或一次性票据。
- 保留现有 HTTP Bearer Token 机制；用 `x-required-scope` 表达业务授权范围，并由契约 CI 校验其与 `TokenScope` 一致。
- 选择整数事件 `sequence` 而不是不透明游标，是为了让重放语义与事件 schema 一致。
- 档案生命周期与隔离语义由 `profile-management` 与 `browser-runtime` 定义；控制面只固定字段、状态码与缺失值表达，并移除 `block_start_on_check_failure` 这类未持久化的阻断开关。
- 控制面不代理 CDP 或 noVNC，避免把高权限入口变成 API 的可复用凭据。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0001-shared-core-two-hosts.md`](../../adr/0001-shared-core-two-hosts.md)
