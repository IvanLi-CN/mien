# 环境档案管理实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 开发前契约已建立，等待核心存储与 API 实现。

## Implementation Coverage

- Requirement coverage: 尚无实现模块；计划由 `mien-core` 的 profile 领域模块、SQLite 迁移和控制面端点覆盖 `REQ-PROFILE-001` 至 `REQ-PROFILE-015`。
- Verification commands: 尚未建立；实现后使用核心单元测试、SQLite 迁移测试、API 集成测试，以及导入 dry-run/提交、导出脱敏与口令、回收站生命周期、下载目录限定和内核迁移的 fixture。
- Rollout facts: 无运行时数据；SQLite schema、迁移规则与错误码已建立，首个实现切片负责落迁移和 fixture；`profiles.deletion_error`、下载目录与 `trash/<id>` 目录布局随首个迁移落地。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 档案 CRUD、分页、分组标签、导入导出、路径安全和脱敏均未实现。
- 导入仍缺 `POST /api/v1/profiles/import/validate` 的零写入 dry-run 与逐项预览（draft/errors/conflict）；`ProfileImportResult` 只属于 commit 端点、`new_id|abort` 冲突策略，以及加密导出/导入的 Argon2id 密钥派生与信封版本。
- 生命周期仍缺 `trash/<id>` 迁移、从回收站恢复、`permanent=true` 先删目录后删行、`deletion_error` 持久化与重试入口。
- 未落地每档案 `downloads` 目录绑定、`copy_fingerprint=true` 新种子派生、顶层 `window` 字段移除和每档案内核迁移（预检/审计/回滚）。
- 数据库迁移工具按 sqlx 方案落地；API 错误码与 OpenAPI 已建立，待实现后补齐迁移与契约测试。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`；当前未实现项为 pending，每行列出计划命令与 fixture。结构检查允许有证据的已实施状态；发布门禁只接受 pass 或有效 waiver。

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-PROFILE-001` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-002` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-003` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-004` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-005` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-006` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-007` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-008` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-009` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-010` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-011` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-012` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-013` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-014` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |
| `VER-PROFILE-015` | `pending` | 核心单元测试、SQLite 迁移测试、导入导出 fixture；fixture 根 `fixtures/profile/` |

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../DATABASE-SCHEMA.md`](../../DATABASE-SCHEMA.md)
- [`../../ERROR-CODES.md`](../../ERROR-CODES.md)
