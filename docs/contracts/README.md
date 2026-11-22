# 机器可读契约

本目录保存主题规格的机器可读补充，便于实现、客户端生成和测试 fixture 使用。规范正文仍以 `docs/specs/` 下的 `SPEC.md` 为准；schema 与正文冲突时，先修正 schema，再同步正文。

| 文件 | 对应主题 | 用途 |
| --- | --- | --- |
| [`fingerprint-spec.schema.json`](fingerprint-spec.schema.json) | [`fingerprint-engine`](../specs/fingerprint-engine/SPEC.md) | 指纹规格校验、UI 表单约束、golden fixture |
| [`kernel-manifest.schema.json`](kernel-manifest.schema.json) | [`browser-runtime`](../specs/browser-runtime/SPEC.md)、[`fingerprint-engine`](../specs/fingerprint-engine/SPEC.md) | 内核能力清单、启动前校验、升级差异比较 |
| [`proxy-engine-manifest.schema.json`](proxy-engine-manifest.schema.json) | [`proxy-gateway`](../specs/proxy-gateway/SPEC.md) | 协议引擎版本、校验和、协议矩阵、许可与升级差异 |
| [`profile-export.schema.json`](profile-export.schema.json) | [`profile-management`](../specs/profile-management/SPEC.md) | 明文配置导出、导入校验；不包含代理凭据、Token、Cookie |

约定：

- 所有 schema 使用 JSON Schema Draft 2020-12。
- 字段命名使用 `snake_case`，与 API 和数据库保持可映射。
- schema 版本通过 `schema_version` 字段表达，不通过文件名后缀表达。
- 导入校验必须拒绝未知字段、越界值和超出大小限制的数组；schema 中的 `additionalProperties: false` 是默认要求。
- 指纹种子、内核版本和 schema 版本是复现指纹的三要素；导出必须保留。
- 代理凭据、UUID、密码、私钥、Token、Cookie、完整分享链接和浏览器数据不属于默认配置导出。
