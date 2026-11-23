# Mien 发布流程

本文固定发布物、签名、许可、SBOM、已知限制和回滚要求。首次发布前，所有门禁必须完成或被项目负责人书面接受。

## 1. 版本与产物

| 产物 | 内容 | 首个发布 |
| --- | --- | --- |
| macOS 应用 | Tauri 2 应用、fingerprint-chromium 148 ARM64、Xray-core v26.11.9 ARM64、更新清单 | 必须 |
| Docker 镜像 | `linux/amd64`、非 root、Xvfb、openbox、noVNC、内核、Xray、健康检查 | 必须 |
| 许可包 | BSD-3-Clause、MPL-2.0、容器组件许可、NOTICE、源码/来源链接 | 必须 |
| SBOM | macOS 应用与 Docker 镜像 | 必须 |
| 测试报告 | 内核、代理、生命周期、安全、UI、性能、镜像 | 必须 |
| 已知限制 | 内核缺陷、伪有头、TUIC、bootstrap、共享引擎故障域 | 必须 |

版本轴独立：

- 应用版本；
- fingerprint-chromium 版本；
- Xray-core 版本；
- 数据库迁移版本；
- 指纹规格 schema 版本；
- 导出 schema 版本；
- 能力清单版本。

## 2. 发布门禁

- `PRD-ACC-001` 至 `PRD-ACC-010` 有当前证据。
- 内核准入 `KERNEL-001` 至 `KERNEL-010` 通过。
- Xray 准入、协议矩阵、共享出站引用计数、连接复用、引擎崩溃重建通过。
- 指纹、DNS、WebRTC、IPv6、DoH、失败关闭测试通过。
- Docker 非 root、只读根、无 Docker socket、沙箱状态可确认。
- 管理 API、运行档案入站、CDP 和 noVNC 不暴露到非必要网络。
- 错误码、事件 schema、OpenAPI、数据库迁移与导出 schema 校验通过。
- macOS 应用签名、公证、Gatekeeper 验证通过。
- Docker 镜像扫毒无未接受的高危/严重漏洞；SBOM 完整。
- MPL-2.0、BSD-3-Clause 与容器内 GPL 组件许可复核通过。
- 项目自身许可证已确定并写入仓库。
- 发布说明列出所有已知限制。

## 3. macOS 发布

1. 构建 `aarch64-apple-darwin`。
2. 下载并校验 fingerprint-chromium 148 DMG；解压内核；计算最终二进制 SHA-256。
3. 下载并校验 Xray-core v26.11.9 macOS ARM64；计算最终二进制 SHA-256。
4. 生成应用、内核、Xray 的签名；验证 Hardened Runtime 与 entitlements。
5. 生成 DMG、自动更新清单、SBOM、许可证与 NOTICE。
6. 公证并 stapler；在干净 macOS 13+ 机器验证首次启动与升级。
7. 保留上一版本用于回滚。

## 4. Docker 发布

1. `buildx` 构建 `linux/amd64`。
2. 固定基础镜像 digest；非 root；只读根；`/data` 唯一可写卷。
3. 镜像内安装 Xvfb、openbox、x11vnc、noVNC、字体、fingerprint-chromium 与 Xray。
4. 启动时校验内核与 Xray SHA-256；失败拒绝启动。
5. 健康检查 `/healthz`；就绪检查 `/readyz`。
6. 生成 SBOM、漏洞扫描、签名或固定 digest。
7. 在 x86_64 主机验证 Xvfb 有头、无头、伪有头、代理矩阵、容量和数据卷。

## 5. 回滚与安全响应

- 内核和 Xray 升级保留上一版本；运行中档案不被静默替换。
- 引擎升级会重置共享连接池；发布说明明确告知用户。
- 发现高危内核或 Xray 缺陷时，先限制受影响版本/协议，再发布修复或回退包。
- 回滚包必须保持签名、数据库迁移兼容和导出 schema 兼容。
- 安全公告包含影响范围、缓解措施和已知限制，不包含利用细节。

## 6. 当前缺口

- Mien 自身项目许可证尚未选择。
- 尚未建立 CI、签名身份、公证账号和 Docker 发布流水线。
- 尚未完成正式法律评审与实际 SBOM/NOTICE 构建。
- M0 Xray、内核、Docker 与代理矩阵 spike 尚未执行。

## 7. 参考

- [`TEST-PLAN.md`](TEST-PLAN.md)
- [`THIRD-PARTY.md`](THIRD-PARTY.md)
- [`SECURITY.md`](SECURITY.md)
- [`ROADMAP.md`](ROADMAP.md)
- [`contracts/manifests/`](contracts/manifests/)
