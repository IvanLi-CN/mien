# 环境档案管理实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 开发前契约已建立，等待核心存储与 API 实现。

## Implementation Coverage

- Requirement coverage: 尚无实现模块；计划由 `mien-core` 的 profile 领域模块、SQLite 迁移和控制面端点覆盖 `REQ-PROFILE-001` 至 `REQ-PROFILE-014`。
- Verification commands: 尚未建立；实现后使用核心单元测试、SQLite 迁移测试、API 集成测试和导入导出 fixture。
- Rollout facts: 无运行时数据；SQLite schema、迁移规则与错误码已建立，首个实现切片负责落迁移和 fixture。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 档案 CRUD、分页、分组标签、导入导出、路径安全和脱敏均未实现。
- 数据库迁移工具按 sqlx 方案落地；API 错误码与 OpenAPI 已建立，待实现后补齐迁移与契约测试。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../DATABASE-SCHEMA.md`](../../DATABASE-SCHEMA.md)
- [`../../ERROR-CODES.md`](../../ERROR-CODES.md)
