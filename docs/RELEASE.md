# Mien 发布流程

本文固定发布物、AdHoc 签名、GitHub Actions 发布、许可、SBOM、已知限制和回滚要求。macOS 不使用 Developer ID 证书，不执行 Apple 公证。首次发布前，所有门禁必须完成或被项目负责人书面接受。

## 1. 版本与产物

| 产物 | 内容 | 首个发布 |
| --- | --- | --- |
| macOS 应用 | Tauri 2 应用、fingerprint-chromium 148 ARM64、Xray-core v26.11.9 ARM64、AdHoc 签名、Chromium JIT entitlements、更新清单 | 必须 |
| macOS 校验 | `SHA256SUMS`、SBOM、许可证（含项目 Apache-2.0）与第三方许可、来源信息、`entitlements.plist`、artifact attestation | 必须 |
| Release identity | `release-identity.json`：policy、target、canonical version、source SHA、reservation ID、artifact digest | 必须 |
| Docker 镜像 | `linux/amd64`、非 root、Xvfb、openbox、noVNC、内核、Xray、健康检查；同一 source 两次构建 digest 相等 | 必须 |
| Docker seccomp | 宿主侧 `mien-seccomp-chromium-<version>.json`；部署者安装后供 Docker daemon 在容器创建前加载 | 必须 |
| 许可包 | 项目 Apache-2.0、BSD-3-Clause、MPL-2.0、容器组件许可、NOTICE、源码/来源链接 | 必须 |
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

发布由维护者在 `release.yml` 中手动输入一个 `release_target` 触发，不由 PR 合并、main push 或 tag push 自动发布。target 可以是 `patch`、`minor`、`major` 或 explicit `X.Y.Z`；workflow 按 [`RELEASE-POLICY.md`](RELEASE-POLICY.md) 解析 canonical version。tag 只在 Release 阶段创建或验证，release workflow 使用受保护的 `release` environment 并要求人工批准。

发布门禁：

- `PRD-ACC-001` 至 `PRD-ACC-010` 全部有当前证据；机器可读测试报告必须为每个强制 ID 标记 `pass`，缺失、失败、跳过或 error 均阻断。
- 产生目标 `main` SHA 的 merged PR 已通过 `CI PR / required`；目标 SHA 本身存在成功的 `CI Main / verify`。
- `release_target` 合法，trusted baseline 可读取；新 identity 的 canonical version 严格大于 baseline，完全匹配的同一 identity recovery 例外。
- trusted baseline 是全部 `vX.Y.Z` tag 与 GitHub Release 的 SemVer 最大值，包含 prerelease 和被标记无效的版本；已失效版本仍占用其版本号。除完全匹配的同一 identity recovery 外，任一已有 tag/release 大于或等于 target 时 fail closed，不允许重发同版本。
- draft reservation 的 version/source 与本次 target 一致；同 version 不同 source 必须冲突。
- 内核准入 `KERNEL-001` 至 `KERNEL-010` 通过。
- Xray 准入、协议矩阵、共享出站引用计数、连接复用、引擎崩溃重建通过。
- 指纹、DNS、WebRTC、IPv6、DoH、失败关闭测试通过。
- Docker 非 root、只读根、无 Docker socket、沙箱状态可确认；镜像从同一 source 独立构建两次且 digest 完全相等，比对结果作为 ACC-010 可复现构建证据。
- 管理 API、运行档案入站、CDP 和 noVNC 不暴露到非必要网络。
- 错误码、事件 schema、OpenAPI、数据库迁移与导出 schema 校验通过。
- GATE-001 机器可读报告通过：所有强制测试 ID 均为 `pass`，没有任何缺失、`fail`、`error` 或 `skipped`。
- macOS 应用与嵌套 Chromium 内核使用且只使用 Chromium JIT 的两项 entitlements（`com.apple.security.cs.allow-jit`、`com.apple.security.cs.allow-unsigned-executable-memory`）；Xray-core 与 frameworks 做 AdHoc 签名但不带 JIT 权限。全部签名通过 `codesign --verify --deep --strict`，`--options runtime` 与 `Signature=adhoc` 已断言；不要求公证或 Gatekeeper 成功。
- `SHA256SUMS`、SBOM、许可证、更新清单、宿主侧 seccomp 发布物和 artifact attestation 完整；attestation 覆盖 DMG、容器 digest、SBOM 与 seccomp 文件，是必选门禁，没有“仓库计划不支持则跳过”的例外。
- Docker 镜像扫描无未接受的高危/严重漏洞；镜像 digest 固定并签名。
- MPL-2.0、BSD-3-Clause 与容器内 GPL 组件许可复核通过。
- 项目自身许可证为 Apache-2.0；仓库根 `LICENSE` 与 `README` 链接就位（M4 门禁）。
- 发布说明列出所有已知限制与 Gatekeeper 手动放行步骤。

## 3. macOS 发布

1. 在 reservation 的 `source_sha` 上于 macOS arm64 runner 构建 `aarch64-apple-darwin`，运行完整 CI。
2. 下载并校验 fingerprint-chromium 148 DMG；解压内核；计算最终二进制 SHA-256。
3. 下载并校验 Xray-core v26.11.9 macOS ARM64；计算最终二进制 SHA-256。
4. 先对 Chromium 内核、Xray 和 frameworks 做 AdHoc 签名，再签应用。Chromium 内核使用 `codesign --force --sign - --timestamp=none --options runtime --entitlements mien-desktop/entitlements.plist`；Xray 与 frameworks 使用同一命令但不传 entitlements；不使用 `--deep` 代替逐个签名。
5. 签应用与 Chromium 内核时传入 `mien-desktop/entitlements.plist`，只授予 Chromium V8 JIT 必需的 `com.apple.security.cs.allow-jit` 与 `com.apple.security.cs.allow-unsigned-executable-memory`。hardened runtime 下缺这两项会导致内核无法启动；不添加调试、网络、文件访问或其他放宽项。
6. 运行 `codesign --verify --deep --strict --verbose=2`；用 `codesign -dv --verbose=4` 断言 `Signature=adhoc` 和 `--options runtime`；用 `codesign -d --entitlements :-` 断言应用与 Chromium 内核只含上述两项，Xray 与 frameworks 不含 JIT 权限。`spctl --assess` 预期拒绝，不作为失败条件。
7. 生成 DMG、`SHA256SUMS`、SBOM、许可证、来源信息、`entitlements.plist` 和 Tauri 更新清单；更新清单使用独立 minisign 密钥签名。
8. 在 `release` job 前，在干净 macOS 13+ 机器下载同一 run 的 DMG 与 `SHA256SUMS`，先校验 SHA-256，再按发布说明手动放行；测试首次启动和升级，结果连同被测 DMG 的 SHA-256 写入 draft 正文，未通过不得公开。任何 macOS 重新构建都使已有手动测试结果作废。
9. 不执行 `notarytool`、`stapler` 或开发者证书签名；不保存 Apple 证书、Apple ID、Team ID 或 App-Specific Password。
10. 保留上一版本用于回滚。

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
4. 启动时校验内核与 Xray SHA-256；失败拒绝启动。seccomp 发布物由部署者安装到宿主路径，容器以该宿主路径启动。
5. 健康检查 `/healthz`；就绪检查 `/readyz`。
6. 从同一 `source_sha` 独立构建两次（清空构建缓存、相同基础镜像 digest 与锁文件），比对两次镜像 digest；不相等即失败，不推送 staging tag；第二次构建不发布。
7. 生成 SBOM、漏洞扫描、容器许可扫描、cosign keyless 签名或固定 digest。
8. 在 x86_64 主机验证 Xvfb 有头、无头、伪有头、代理矩阵、容量、seccomp 宿主加载和数据卷。

## 5. GitHub Actions 发布流程

发布 workflow 为 `.github/workflows/release.yml`，完整设计见 [`CI.md`](CI.md)，版本与 identity 见 [`RELEASE-POLICY.md`](RELEASE-POLICY.md)：

所有 job 从 reservation 读取 `source_sha` 并 checkout 该 SHA，不使用 main HEAD。

1. `resolve-target` 读取 `release_target` 和 trusted baseline（全部 `vX.Y.Z` tag 和 release 的 SemVer 最大值，包含 prerelease 与已失效版本），先读取已有 reservation/identity，再执行 collision 检查；校验 `main` source 和 `CI Main / verify`；完全匹配的同一 identity 允许 recovery，其他任何 tag/release 大于或等于 target 时 fail closed。
2. `reserve` 创建或复用 draft GitHub Release，写入 policy、target、version、source SHA 和 run ID。
3. `test` 调用共享 CI workflow，运行完整测试和 e2e；不接触 release secrets。
4. `macos-arm64` 与 `docker-amd64` 并行构建，分别上传 DMG、SBOM、校验和、宿主侧 seccomp 发布物与镜像 digest；`docker-amd64` 必须双构建且 digest 相等，第二次构建只做可复现性验证，不发布。
5. `stage-release` 把产物和 seccomp 发布物写入 draft，校验 SHA-256/digest，写入 `release-identity.json`；本次构建 digest 与 identity 已记录 digest 不一致时拒绝。
6. `attest` 为 DMG、容器 digest 和 seccomp 发布物生成 GitHub artifact attestation；没有降级或跳过分支。
7. 阶段 7 `promote-docker` 把 staging digest 提升为 `vX.Y.Z` GHCR tag；绑定 `release` environment。
8. 阶段 8 `release` 校验 identity 后创建或验证 tag，公开 release；绑定 `release` environment。
9. `verify-release` 下载公开资产，校验 checksums、SBOM、seccomp 发布物、AdHoc 签名、entitlements、更新清单和 identity；失败时标记 release 无效。

失败后用相同 `release_target` 和相同 `source_sha` 重跑，checkout reservation 的 `source_sha`，复用已绑定 canonical version、draft reservation 和已记录 digest；identity 已记录 digest 的产物不再构建，不重新分配版本，不创建 successor，不改写已有 tag。重新构建 macOS 产物会使手动测试证据作废，必须重做。需要修正构建时选择新的 `release_target`，产生新 version；release 资产不可变，回滚使用上一版本 release，不覆盖已发布文件。

## 6. 回滚与安全响应

- 内核和 Xray 升级保留上一版本；运行中档案不被静默替换。
- 引擎升级会重置共享连接池；发布说明明确告知用户。
- 发现高危内核或 Xray 缺陷时，先限制受影响版本/协议，再发布修复或回退包。
- 回滚包必须保持 AdHoc 签名/校验和、数据库迁移兼容和导出 schema 兼容。
- 已公开 release 的 `verify-release` 失败时，立即标记 release 无效并通知维护者。
- 被标记无效的版本仍占用其版本号；修复不能重发同版本，必须选择新的 `release_target` 产生新版本。
- 安全公告包含影响范围、缓解措施和已知限制，不包含利用细节。

## 7. 当前缺口

- 项目许可已定为 Apache-2.0；`LICENSE` 文本与 `README` 许可链接由产品文档负责，落地后才算 M4 许可门禁通过。
- `.github/workflows/` 的实际 YAML、branch protection、merge queue、`release` environment、draft reservation、不带 bypass list 的 tag ruleset、Docker 双构建比对和强制 attestation 尚未落地。
- entitlements 文件与签名参数尚未在实际打包中验证。
- Tauri 更新签名密钥尚未生成和配置。
- 尚未完成正式法律评审与实际 SBOM/NOTICE 构建。
- M0 Xray、内核、Docker 与代理矩阵 spike 尚未执行。

## 8. 参考

- [`CI.md`](CI.md)
- [`RELEASE-POLICY.md`](RELEASE-POLICY.md)
- [`TEST-PLAN.md`](TEST-PLAN.md)
- [`THIRD-PARTY.md`](THIRD-PARTY.md)
- [`SECURITY.md`](SECURITY.md)
- [`ROADMAP.md`](ROADMAP.md)
- [`adr/0010-adhoc-macos-distribution.md`](adr/0010-adhoc-macos-distribution.md)
- [`adr/0011-manual-release-identity.md`](adr/0011-manual-release-identity.md)
- [`contracts/manifests/`](contracts/manifests/)
