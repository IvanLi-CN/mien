# 指纹规格实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 指纹 schema、能力清单与参数构建器的契约已建立。

## Implementation Coverage

- Requirement coverage: 计划由 `mien-core` 的 fingerprint 模块覆盖 `REQ-FP-001` 至 `REQ-FP-016`；能力清单由内核注册表提供，探针由测试工具提供。
- Verification commands: 尚未建立；实现后使用 golden argv fixture、schema 属性测试、种子冲突测试和内核准入探针。
- Rollout facts: 首个实现基于 fingerprint-chromium 148 的能力清单；宿主绑定字段不进入规格。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 规格 schema、校验器、种子生成、参数构建器、迁移和探针分类均未实现。
- 需要先确定 148 的实际能力清单与探针基线，再冻结 golden fixture。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
