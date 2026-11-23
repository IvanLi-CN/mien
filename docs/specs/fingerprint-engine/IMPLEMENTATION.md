# 指纹规格实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 指纹 schema、能力清单与参数构建器的契约已建立；窗口尺寸、可选 `spoof`、`sandbox_kind`、`network_controls=unsupported` 与安装记录双摘要语义已纳入契约。

## Implementation Coverage

- Requirement coverage: 计划由 `mien-core` 的 fingerprint 模块覆盖 `REQ-FP-001` 至 `REQ-FP-020`；能力清单由内核注册表提供，探针由测试工具提供。`REQ-FP-001`/`002`/`007`/`009`/`011` 覆盖窗口规范位置、种子生成、清单驱动取值与可选 `spoof`；`REQ-FP-019`/`020` 覆盖 `sandbox_kind` 驱动的沙箱校验与安装记录 `binary_sha256` 校验。
- Verification commands: 实现后使用 golden argv fixture（含 `window-size` 只出现一次）、schema 属性测试、复制与导入种子测试、带 `per_platform_values`/`unsupported`/`sandbox_kind=none` 的清单 fixture、沙箱校验顺序测试与可执行文件摘要校验测试；清单 schema 由 Ajv 编译，`window-size` 缺失即清单无效。
- Rollout facts: 首个实现基于 fingerprint-chromium 148 的能力清单；宿主绑定字段不进入规格；Linux 的 `sandbox_kind=namespace` 需要强制 seccomp profile，Windows 清单把 DoH/IPv6 控制声明为 `unsupported` 作为有效的能力缺失 fixture。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 规格 schema、校验器、种子生成、参数构建器、迁移和探针分类均未实现。
- 需要先确定 148 的实际能力清单与探针基线，再冻结 golden fixture；窗口尺寸只允许来自指纹规格，档案 payload、启动偏好与导出不得新增顶层 `window`。
- 沙箱校验顺序、`isolation` 结论与安装记录摘要校验需要与 `browser-runtime` 实现同步接入。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../contracts/manifests/`](../../contracts/manifests/)
- [`../../contracts/fingerprint-spec.schema.json`](../../contracts/fingerprint-spec.schema.json)
- [`../../contracts/kernel-manifest.schema.json`](../../contracts/kernel-manifest.schema.json)
- [`../../UI-SURFACES.md`](../../UI-SURFACES.md)
