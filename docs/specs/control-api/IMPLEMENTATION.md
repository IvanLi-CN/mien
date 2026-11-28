# 控制面 API 实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始（服务端）：仓库只有 `mien-core` 库与 Tauri 桌面宿主，没有 HTTP/WS 控制面；仅 OpenAPI/事件/错误码契约与 `ErrorCode` 目录校验落地。
- Lifecycle: active
- Catalog note: 服务端认证、Token、事件、OpenAPI、事件 schema 与错误码契约已建立；公开白名单、`ticketAuth`、HTTP Bearer 的 `x-required-scope`、日志尾部与导入导出端点已纳入契约。

## Implementation Coverage

- Requirement coverage: `REQ-API-001` 至 `REQ-API-021` 计划由 `mien-server` 的路由、认证、WebSocket 与静态资源模块覆盖；该 crate 尚不存在。当前可复跑的实现只有 `ErrorCode` 枚举与 `docs/ERROR-CODES.md` 的双向一致性校验，以及 `docs/contracts/openapi.yaml` 的可编译性。
- Verification commands: 唯一可复跑的证据见 `## M2 slice evidence`（契约与错误码校验）；HTTP 层不存在，所有 VER 保持 `pending`。
- Rollout facts: 首个实现仅服务端提供 HTTP 控制面；桌面自动化 API 默认关闭。重放窗口在 SPEC 固定为 7 天与 100 MiB payload 上限，回放实现必须使用一致的 floor/high_water 快照并拒绝窗口外游标。

## Coverage / rollout summary

- `xtask contracts` 通过：OpenAPI 与 events/fingerprint 等 schema 可解析，Vite 前端类型仍能生成。
- `ErrorCode` 与 `docs/ERROR-CODES.md` 完全一致，且每个码可往返解析。

## Remaining Gaps

- 无服务端进程：路由、会话、Token、CSRF/Origin、幂等、一次性票据、健康检查、审计与代理 API 全部缺失。
- 桌面宿主只暴露 Tauri IPC 命令（`profiles_*`、`runtime_*`），没有 HTTP 端点，也没有 OpenAPI generation-diff 作业。
- `x-required-scope` 的 CI 校验（必须属于 `TokenScope`）未接入。
- 日志尾部、导入导出端点与 `ProfileImportResult`/`ProfileExportEnvelope` 未实现。
- 运行时状态不落库，`REQ-API-021` 依赖的六态持久化、`last_known_isolation` 与 `deletion_error` 只存在于迁移与内存。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`。本表的 `pass` 只表示“仓库内存在可运行命令且 T19 亲自运行通过”，并写出确切命令；其余保持 `pending` 并写明缺什么。发布门禁只接受 pass 或有效 waiver。

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-API-001` | `pending` | 无 HTTP 路由，版本前缀与弃用头无从验证。fixture 根 `fixtures/api/` |
| `VER-API-002` | `pending` | `cargo run -q -p xtask -- contracts` 可证明 OpenAPI 可解析，但运行时端点不存在，“重新生成无 diff”与生成客户端可编译均未建立。fixture 根 `fixtures/api/` |
| `VER-API-003` | `pending` | 无认证中间件与公开白名单。fixture 根 `fixtures/api/` |
| `VER-API-004` | `pending` | 桌面自动化只经 Tauri IPC，回环 HTTP 与 Token 撤销不存在。fixture 根 `fixtures/api/` |
| `VER-API-005` | `pending` | Token 表、HMAC 哈希与 `x-required-scope` 校验未实现。fixture 根 `fixtures/api/` |
| `VER-API-006` | `pending` | 无 CSRF/Origin 中间件。fixture 根 `fixtures/api/` |
| `VER-API-007` | `pending` | 无登录与限流。fixture 根 `fixtures/api/` |
| `VER-API-008` | `pending` | `cargo test -p mien-core --lib error` 证明 `ErrorCode` 与 `docs/ERROR-CODES.md` 双向一致、每个码可往返；缺 HTTP 错误结构与 OpenAPI/事件 schema 的双向比对作业与漂移注入用例。fixture 根 `fixtures/api/` |
| `VER-API-009` | `pending` | 无审计表写入与游标分页端点。fixture 根 `fixtures/api/` |
| `VER-API-010` | `pending` | 无幂等键存储。fixture 根 `fixtures/api/` |
| `VER-API-011` | `pending` | 事件重放窗口、`ticketAuth`、一次性票据与 floor/high_water 均未实现。fixture 根 `fixtures/api/` |
| `VER-API-012` | `pending` | 无 `/healthz`、`/readyz`。fixture 根 `fixtures/api/` |
| `VER-API-013` | `pending` | 无请求体上限与超时处理。fixture 根 `fixtures/api/` |
| `VER-API-014` | `pending` | 无审计事件产生与查询。fixture 根 `fixtures/api/` |
| `VER-API-015` | `pending` | 无 HTTP 响应头，CSP/`X-Content-Type-Options` 等未设置。fixture 根 `fixtures/api/` |
| `VER-API-016` | `pending` | 无查看器端点，CDP 也只在桌面本地。fixture 根 `fixtures/api/` |
| `VER-API-017` | `pending` | 代理 CRUD 只存在于 `mien-core` 与测试，无 HTTP 端点；分享链接解析有 `cargo test -p mien-core --test share_link` 但无 API 层。fixture 根 `fixtures/api/` |
| `VER-API-018` | `pending` | 无登录与首次改密流程。fixture 根 `fixtures/api/` |
| `VER-API-019` | `pending` | 日志尾部只存在于 `RuntimeSupervisor` 内存，无端点与 `limit` 校验。fixture 根 `fixtures/api/` |
| `VER-API-020` | `pass` | `cargo test -p mien-server --test export_import`（8 项）：明文载荷逐键独立校验 `profile-export.schema.json` 且无凭据值/顶层 `window`/秘密键；plain+passphrase 与 encrypted 缺 passphrase 均 422 且字段指向 `passphrase`；加密往返后指纹逐字段相等；错误口令与篡改容器只报字段名且不回显口令；`validate` 前后档案行数、`profiles/` 条目与 `runtimes/imports/` 清单数完全相等；`abort` 409 且零写入、`new_id` 两次导入两行且 `import_source_id` 相同；第二项写入失败时首行与目录全部回滚；口令不出现在 tracing 输出、审计与事件载荷。fixture 根 `fixtures/api/` |
| `VER-API-021` | `pending` | 运行时六态不落库（`runtimes` 表无写入路径），`isolation`/`last_known_isolation` 与 `deletion_error` 无法经 API 观察；`block_start_on_check_failure` 缺失这一点可由契约检查佐证，但不足以判定整行 pass。fixture 根 `fixtures/api/` |

## M1 slice evidence

- Nothing delivered yet; the M1 slice is library-level only. REST/WS, sessions, tokens, CSRF and audit remain pending.

## M2 slice evidence

- Re-run in T19 on 2026-11-27:
  - `cargo test -p mien-core --lib error` — 4 passed（目录双向一致、码往返、稳定码 + 人类消息）。
  - `cargo run -q -p xtask -- contracts` — OpenAPI 与 schema 可解析（`--skip-host` 全量检查的子步骤）。
- No API row has an HTTP-level command; every VER stays `pending`.

## T28 slice evidence

- 导入导出端点（`POST /api/v1/profiles/{id}/export`、`/import/validate`、`/import`）：
  - 控制面：`crates/mien-server/src/profiles/{export_import,payload,container}.rs`；领域事务与目录清理：`crates/mien-core/src/storage/profile.rs`。
  - `cargo test -p mien-server --test export_import` — 8 passed。
  - `cargo test -p mien-server` — 47 + 13 + 12 + 8 + 12 + 18 + 5 + 6 + 8 passed。
  - 无新迁移：`profiles.import_source_id` 自 `0002_profile_base.sql` 起已存在。
  - 无新 crate：加密用既有 `aes-gcm`/`argon2`/`zeroize`（`zeroize` 与 `aes-gcm` 加入 `mien-server` 的既有 workspace 依赖引用，未引入版本）。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../contracts/openapi.yaml`](../../contracts/openapi.yaml)
- [`../../contracts/events.schema.json`](../../contracts/events.schema.json)
- [`../../ERROR-CODES.md`](../../ERROR-CODES.md)

最近同步：M2（T19），依据 `cargo test -p mien-core --lib error` 与 `cargo run -q -p xtask -- check --skip-host`（含 `xtask contracts`）；服务端尚未实现，故 21 条 VER 全部保持 `pending`。
