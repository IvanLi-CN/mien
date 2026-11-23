# Mien 发布流程

本文固定发布物、AdHoc 签名、GitHub Actions 发布、许可、SBOM、已知限制和回滚要求。macOS 不使用 Developer ID 证书，不执行 Apple 公证。首次发布前，所有门禁必须完成或被项目负责人书面接受。

## 1. 版本与产物

| 产物 | 内容 | 首个发布 |
| --- | --- | --- |
| macOS 应用 | Tauri 2 应用、fingerprint-chromium 148 ARM64、Xray-core v26.11.9 ARM64、AdHoc 签名、更新清单 | 必须 |
| macOS 校验 | `SHA256SUMS`、SBOM、许可证、来源信息、artifact attestation（计划支持时） | 必须 |
| Docker 镜像 | `linux/amd64`、非 root、Xvfb、openbox、noVNC、内核、Xray、健康检查 | 必须 |
| 许可包 | BSD-3-Clause、MPL-2.0、容器组件许可、NOTICE、源码/来源链接 | 必须 |
| SBOM | macOS 应用与 Docker 镜像 | 必须 |
| 测试报告 | 内核、代理、生命周期、安全、UI、性能、镜像 | 必须 |
| 已知限制 | 内核缺陷、伪有头、TUIC、bootstrap、共享引擎故障域、Gatekeeper 行为 | 必须 |

版本轴独立：

- 应用版本；
- fingerprint-chromium 版本；
- Xray-core 版本；
- 数据库迁移版本；
- 指纹规格 schema 版本；
- 导出 schema 版本；
- 能力清单版本。

## 2. 发布触发与门禁

发布由维护者推送 `vX.Y.Z` tag 触发，不由 PR 合并或 main push 自动发布。tag 必须指向已通过 main CI 的提交，release workflow 使用受保护的 `release` environment 并要求人工批准。

发布门禁：

- `PRD-ACC-001` 至 `PRD-ACC-010` 有当前证据。
- `CI PR / required` 在目标提交上通过；mainline 完整验证通过。
- 内核准入 `KERNEL-001` 至 `KERNEL-010` 通过。
- Xray 准入、协议矩阵、共享出站引用计数、连接复用、引擎崩溃重建通过。
- 指纹、DNS、WebRTC、IPv6、DoH、失败关闭测试通过。
- Docker 非 root、只读根、无 Docker socket、沙箱状态可确认。
- 管理 API、运行档案入站、CDP 和 noVNC 不暴露到非必要网络。
- 错误码、事件 schema、OpenAPI、数据库迁移与导出 schema 校验通过。
- macOS 应用、嵌套内核与 Xray-core 的 AdHoc 签名通过 `codesign --verify`；`Signature=adhoc`；不要求公证或 Gatekeeper 成功。
- `SHA256SUMS`、SBOM、许可证、更新清单和 attestation（支持时）完整。
- Docker 镜像扫描无未接受的高危/严重漏洞；镜像 digest 固定并签名。
- MPL-2.0、BSD-3-Clause 与容器内 GPL 组件许可复核通过。
- 项目自身许可证已确定并写入仓库。
- 发布说明列出所有已知限制与 Gatekeeper 手动放行步骤。

## 3. macOS 发布

1. 在 macOS arm64 runner 构建 `aarch64-apple-darwin`，运行完整 CI。
2. 下载并校验 fingerprint-chromium 148 DMG；解压内核；计算最终二进制 SHA-256。
3. 下载并校验 Xray-core v26.11.9 macOS ARM64；计算最终二进制 SHA-256。
4. 先对内核、Xray 和 frameworks 做 AdHoc 签名，再签应用：`codesign --force --sign - --timestamp=none --options runtime`；不使用 `--deep` 代替逐个签名。
5. 运行 `codesign --verify --deep --strict --verbose=2`；用 `codesign -dv --verbose=4` 断言 `Signature=adhoc`。`spctl --assess` 预期拒绝，不作为失败条件。
6. 生成 DMG、`SHA256SUMS`、SBOM、许可证、来源信息和 Tauri 更新清单；更新清单使用独立 minisign 密钥签名。
7. 在干净 macOS 13+ 机器下载发布资产，先校验 SHA-256 或 attestation，再按发布说明手动放行；测试首次启动和升级。
8. 不执行 `notarytool`、`stapler` 或开发者证书签名；不保存 Apple 证书、Apple ID、Team ID 或 App-Specific Password。
9. 保留上一版本用于回滚。

发布说明必须写明：

- 应用没有 Apple Developer ID 签名，也没有公证。
- 首次启动会被 Gatekeeper 阻止。
- 用户应先核对 `SHA256SUMS` 或 artifact attestation，再通过“系统设置 → 隐私与安全性 → 仍要打开”或用 `xattr -dr com.apple.quarantine /Applications/Mien.app` 手动放行。
- AdHoc 签名不能证明发布者身份；GitHub Release 账户和制品证明才是分发信任边界。
- 不要关闭系统级 Gatekeeper，也不要在未校验资产的情况下放行。

## 4. Docker 发布

1. `buildx` 构建 `linux/amd64`。
2. 固定基础镜像 digest；非 root；只读根；`/data` 唯一可写卷。
3. 镜像内安装 Xvfb、openbox、x11vnc、noVNC、字体、fingerprint-chromium 与 Xray。
4. 启动时校验内核与 Xray SHA-256；失败拒绝启动。
5. 健康检查 `/healthz`；就绪检查 `/readyz`。
6. 生成 SBOM、漏洞扫描、cosign keyless 签名或固定 digest。
7. 在 x86_64 主机验证 Xvfb 有头、无头、伪有头、代理矩阵、容量和数据卷。

## 5. GitHub Actions 发布流程

发布 workflow 为 `.github/workflows/release.yml`，完整设计见 [`CI.md`](CI.md)：

1. `verify-tag` 校验 SemVer、tag 指向 main 祖先提交、版本号一致，且 release 尚不存在。
2. `test` 调用共享 CI workflow，运行完整测试和 e2e；不接触 release secrets。
3. `macos-arm64` 与 `docker-amd64` 并行构建，分别上传 DMG、SBOM、校验和与镜像 digest。
4. `publish` 在 `release` environment 批准后创建 draft GitHub Release，上传资产。
5. `attest` 为 DMG 和容器 digest 生成 GitHub artifact attestation（计划支持时）。
6. `verify-published` 下载已上传资产，校验 checksums、AdHoc 签名、更新清单和 DMG 内容。
7. `finalize` 仅在全部前置验证成功后公开 release。

release workflow 不自动创建 tag；失败后对同一 tag 重跑，不创建内容相同的新 tag。release 资产不可变，回滚使用上一版本 release，不覆盖已发布文件。

## 6. 回滚与安全响应

- 内核和 Xray 升级保留上一版本；运行中档案不被静默替换。
- 引擎升级会重置共享连接池；发布说明明确告知用户。
- 发现高危内核或 Xray 缺陷时，先限制受影响版本/协议，再发布修复或回退包。
- 回滚包必须保持 AdHoc 签名/校验和、数据库迁移兼容和导出 schema 兼容。
- 已公开 release 的 `verify-published` 失败时，立即标记 release 无效并通知维护者。
- 安全公告包含影响范围、缓解措施和已知限制，不包含利用细节。

## 7. 当前缺口

- Mien 自身项目许可证尚未选择。
- `.github/workflows/` 的实际 YAML、branch protection、merge queue、`release` environment 和 tag 保护尚未落地。
- Tauri 更新签名密钥尚未生成和配置。
- 尚未完成正式法律评审与实际 SBOM/NOTICE 构建。
- M0 Xray、内核、Docker 与代理矩阵 spike 尚未执行。

## 8. 参考

- [`CI.md`](CI.md)
- [`TEST-PLAN.md`](TEST-PLAN.md)
- [`THIRD-PARTY.md`](THIRD-PARTY.md)
- [`SECURITY.md`](SECURITY.md)
- [`ROADMAP.md`](ROADMAP.md)
- [`adr/0010-adhoc-macos-distribution.md`](adr/0010-adhoc-macos-distribution.md)
- [`contracts/manifests/`](contracts/manifests/)
