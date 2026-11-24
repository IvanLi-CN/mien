# 控制面 API 实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 服务端认证、Token、事件、OpenAPI、事件 schema 与错误码契约已建立；公开白名单、`ticketAuth`、HTTP Bearer 的 `x-required-scope`、日志尾部与导入导出端点已纳入契约。

## Implementation Coverage

- Requirement coverage: 计划由 `mien-server` 的路由、认证、WebSocket 与静态资源模块覆盖 `REQ-API-001` 至 `REQ-API-021`；登录与 Token、一次性票据、幂等、审计筛选、有界日志尾部、导入导出与档案/运行时 payload 字段分别对应 `REQ-API-003`/`005`/`010`/`011`/`009`/`019`/`020`/`021`；代理 CRUD、分享链接导入与检查复用共享 Xray 引擎能力清单（`REQ-API-017`）；桌面宿主通过 Tauri IPC 镜像命令语义。
- Verification commands: 实现后使用 OpenAPI 生成与运行同一契约的 generation-diff 作业（`REQ-API-002`）；`ErrorCode` 目录与 OpenAPI/事件 schema 枚举的双向比对（`REQ-API-008`）；API 集成测试覆盖公开白名单、401/403/404 优先级、CSRF/限流、一次性票据与 `after=<sequence>` 重放、`tokensCreate` 幂等冲突、审计筛选、日志尾部脱敏、导入导出与 payload 状态字段。
- Rollout facts: 首个实现仅服务端提供 HTTP 控制面；桌面自动化 API 默认关闭。WebSocket 重放窗口上限在实现时记录，窗口外行为必须写入本文件而不是新增未声明错误码。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 登录、首次改密、会话、Token scope、CSRF/Origin、错误结构、分页、幂等、一次性票据、健康检查、审计和代理 API 均未实现。
- 公开白名单、401/403/404 优先级、`ticketAuth` 与 `x-required-scope` 已写入 OpenAPI；生成与双向枚举比对作业尚未接入 CI。
- 有界日志尾部端点、导入导出端点与 `ProfileImportResult`/`ProfileExportEnvelope` 尚未实现；`block_start_on_check_failure` 必须在实现与 schema 中保持不存在。
- OpenAPI、事件 schema 与错误码已建立；实现后需要用生成客户端和 fixture 建立契约测试。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../contracts/openapi.yaml`](../../contracts/openapi.yaml)
- [`../../contracts/events.schema.json`](../../contracts/events.schema.json)
- [`../../ERROR-CODES.md`](../../ERROR-CODES.md)
