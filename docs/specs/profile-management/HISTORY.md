# 环境档案管理主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题在开发启动前建立，作为档案能力的长期契约。
- 导入导出 schema 与应用版本解耦，后续通过独立版本号演进。
- 数据库 schema 迁移必须保持向前兼容和可回滚，具体策略在实现阶段记录。

## Replacements / Background

- 档案管理被拆分为独立主题，避免把指纹语义、代理协议和运行时监督混入同一契约。
- 采用回收站式软删除而不是立即清除：非永久删除把目录移入 `<data-root>/trash/<id>`，恢复时移回；彻底删除先删目录后删行，失败保留 `deleted` 并记录 `deletion_error`，以降低误删风险并保留重试证据。
- 默认导出不包含任何秘密，加密导出使用口令派生密钥；浏览器 Cookie 与站点数据不进入配置导出；格式与口令只经由请求体传递，导入先 dry-run 再提交。
- 窗口尺寸与指纹规格一起存储，因为窗口是指纹可复现性的一部分；复制指纹时总是重新生成种子，避免两个档案共享派生特征。
- 每档案内核版本是显式绑定，迁移需要预检、审计和回滚；安装记录区分下载资产摘要与解包后可执行文件摘要。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0003-storage-and-secret-protection.md`](../../adr/0003-storage-and-secret-protection.md)
