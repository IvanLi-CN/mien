# 指纹规格主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题在开发启动前建立，绑定 fingerprint-chromium 148 的已文档化开关。
- 规格 schema 与内核版本解耦，但校验必须读取具体内核能力清单。
- 内核升级可能改变派生特征；迁移必须保留原内核、种子和规格版本用于回滚。

## Replacements / Background

- 指纹模型选择“显式字段 + 种子派生”，拒绝自由 User-Agent 和逐项噪声控制，以避免 UA、UA-CH、内核版本和派生特征之间产生矛盾。
- 屏幕尺寸、设备像素比、颜色深度和字体白名单被归为宿主绑定字段，不暴露为可伪造能力。
- 上游存在无效果开关；能力清单和探针用于阻止产品展示这些能力。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0002-pin-kernel-148.md`](../../adr/0002-pin-kernel-148.md)
- [`../../adr/0006-fingerprint-spec-model.md`](../../adr/0006-fingerprint-spec-model.md)
