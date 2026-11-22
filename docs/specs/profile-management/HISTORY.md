# 环境档案管理主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题在开发启动前建立，作为档案能力的长期契约。
- 导入导出 schema 与应用版本解耦，后续通过独立版本号演进。
- 数据库 schema 迁移必须保持向前兼容和可回滚，具体策略在实现阶段记录。

## Replacements / Background

- 档案管理被拆分为独立主题，避免把指纹语义、代理协议和运行时监督混入同一契约。
- 采用软删除与保留窗口，是为了降低误删风险，同时允许用户显式永久删除。
- 默认导出不包含任何秘密，加密导出使用口令派生密钥；浏览器 Cookie 与站点数据不进入配置导出。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0003-storage-and-secret-protection.md`](../../adr/0003-storage-and-secret-protection.md)
