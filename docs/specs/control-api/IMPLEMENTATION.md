# 控制面 API 实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 服务端认证、Token、事件、OpenAPI、事件 schema 与错误码契约已建立；公开白名单、`ticketAuth`、HTTP Bearer 的 `x-required-scope`、日志尾部与导入导出端点已纳入契约。

## Implementation Coverage

- Requirement coverage: 计划由 `mien-server` 的路由、认证、WebSocket 与静态资源模块覆盖 `REQ-API-001` 至 `REQ-API-021`；登录与 Token、一次性票据、幂等、审计筛选、有界日志尾部、导入导出与档案/运行时 payload 字段分别对应 `REQ-API-003`/`005`/`010`/`011`/`009`/`019`/`020`/`021`；代理 CRUD、分享链接导入与检查复用共享 Xray 引擎能力清单（`REQ-API-017`）；桌面宿主通过 Tauri IPC 镜像命令语义。
- Verification commands: 实现后使用 OpenAPI 生成与运行同一契约的 generation-diff 作业（`REQ-API-002`）；`ErrorCode` 目录与 OpenAPI/事件 schema 枚举的双向比对（`REQ-API-008`）；API 集成测试覆盖公开白名单、401/403/404 优先级、CSRF/限流、一次性票据与 `after=<sequence>` 重放、`tokensCreate` 幂等冲突、审计筛选、日志尾部脱敏、导入导出与 payload 状态字段。
- Rollout facts: 首个实现仅服务端提供 HTTP 控制面；桌面自动化 API 默认关闭。重放窗口在 SPEC 固定为 7 天与 100 MiB payload 上限，回放实现必须使用一致的 floor/high_water 快照并拒绝窗口外游标。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 登录、首次改密、会话、Token scope、CSRF/Origin、错误结构、分页、幂等、一次性票据、健康检查、审计和代理 API 均未实现。
- 公开白名单、401/403/404 优先级、`ticketAuth` 与 `x-required-scope` 已写入 OpenAPI；生成与双向枚举比对作业尚未接入 CI。
- 有界日志尾部端点、导入导出端点与 `ProfileImportResult`/`ProfileExportEnvelope` 尚未实现；`block_start_on_check_failure` 必须在实现与 schema 中保持不存在。
- OpenAPI、事件 schema 与错误码已建立；实现后需要用生成客户端和 fixture 建立契约测试。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`；当前未实现项为 pending，每行列出计划命令与 fixture。结构检查允许有证据的已实施状态；发布门禁只接受 pass 或有效 waiver。

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-API-001` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-002` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-003` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-004` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-005` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-006` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-007` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-008` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-009` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-010` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-011` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-012` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-013` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-014` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-015` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-016` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-017` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-018` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-019` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-020` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |
| `VER-API-021` | `pending` | OpenAPI 契约测试、HTTP 集成测试、CSRF/Origin 测试、事件回放测试；fixture 根 `fixtures/api/` |

## M1 slice evidence

- Nothing delivered yet; the M1 slice is library-level only. REST/WS, sessions, tokens, CSRF and audit remain pending.

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../contracts/openapi.yaml`](../../contracts/openapi.yaml)
- [`../../contracts/events.schema.json`](../../contracts/events.schema.json)
- [`../../ERROR-CODES.md`](../../ERROR-CODES.md)
