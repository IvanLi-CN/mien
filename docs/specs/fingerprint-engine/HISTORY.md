# 指纹规格主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题在开发启动前建立，绑定 fingerprint-chromium 148 的已文档化开关。
- 规格 schema 与内核版本解耦，但校验必须读取具体内核能力清单。
- 内核升级可能改变派生特征；迁移必须保留原内核、种子和规格版本用于回滚。
- 窗口尺寸 `window` 只在指纹规格内保存，映射为清单声明的 `window-size`（`type: string`，`WIDTH,HEIGHT`）；档案顶层字段、启动偏好、导出 `runtime` 与 `window_json` 列均已移除。
- `spoof` 及其类别为可选字段，省略即启用；只有显式 `false` 才生成 `--disable-spoofing`。
- 复制档案（`copy_fingerprint=true`）总是生成新种子；种子冲突不再是错误路径，导入保留导出种子以保证可复现。
- 能力清单用 `sandbox_kind`（`namespace|seatbelt|win32-restricted-token|none`）取代 `requires_namespace_sandbox` 布尔；`network_controls` 允许 `disabled|unsupported` 与 `block|unsupported`，缺失能力的清单是合法 fixture。
- `flagSpec.per_platform_values` 为 CPU、语言、时区等平台限制提供数据驱动来源；声明时优先于 `values`，不得合并放宽。
- 安装记录同时保存 `asset_sha256` 与 `binary_sha256`；运行时只以解包后可执行文件的 `binary_sha256` 校验，清单 `sha256` 仅代表发布资产摘要。

## Replacements / Background

- 指纹模型选择“显式字段 + 种子派生”，拒绝自由 User-Agent 和逐项噪声控制，以避免 UA、UA-CH、内核版本和派生特征之间产生矛盾。
- 屏幕尺寸、设备像素比、颜色深度、字体白名单与 `navigator.plugins` 被归为宿主绑定字段，不暴露为可伪造能力。
- 上游存在无效果开关；能力清单和探针用于阻止产品展示这些能力。
- 选择 `sandbox_kind` 而不是布尔值，是为了让 Linux、macOS、Windows 的沙箱机制在契约中可区分，并避免把“声明”当成“已生效”；有效沙箱仍需启动前校验。
- 选择窗口作为指纹规格字段而不是运行偏好，是为了让同一规格可复现并避免两处窗口尺寸互相矛盾。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0002-pin-kernel-148.md`](../../adr/0002-pin-kernel-148.md)
- [`../../adr/0006-fingerprint-spec-model.md`](../../adr/0006-fingerprint-spec-model.md)
