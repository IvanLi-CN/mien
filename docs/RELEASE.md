# Mien 发布流程

本文固定发布物、AdHoc 签名、GitHub Actions 发布、许可、SBOM、已知限制和回滚要求。macOS 不使用 Developer ID 证书，不执行 Apple 公证。首次发布前，所有强制门禁必须完成；只有 [`TEST-PLAN.md`](TEST-PLAN.md) 允许逐项具名接受的 P1 才能按该规则接受，P0 与阻断类 P1 不得整类或按环境豁免。

## 1. 版本与产物

| 产物 | 内容 | 首个发布 |
| --- | --- | --- |
| macOS 应用 | Tauri 2 应用、fingerprint-chromium 148 ARM64、Xray-core v26.11.9 ARM64、AdHoc 签名、Chromium JIT entitlements、更新清单 | 必须 |
| macOS 校验 | `SHA256SUMS`、SBOM、`LICENSE`、`NOTICE`、`LICENSES/`（含项目 Apache-2.0 与第三方许可）、来源信息、`entitlements.plist`、artifact attestation | 必须 |
| Release identity | `release-identity.json`：policy、target、canonical version、source SHA、reservation ID、artifact digest | 必须 |
| Docker 镜像 | `linux/amd64`、非 root、Xvfb、openbox、noVNC、内核、Xray、健康检查；同一 source 两次构建 digest 相等 | 必须 |
| Docker seccomp | 宿主侧 `mien-seccomp-chromium-<version>.json`；部署者安装后供 Docker daemon 在容器创建前加载 | 必须 |
| 许可包 | 项目 Apache-2.0、BSD-3-Clause、MPL-2.0、容器组件许可、NOTICE、LICENSES/、源码/来源链接 | 必须 |
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

- PRD-ACC-001 至 PRD-ACC-010 全部有当前证据且必须 pass，不接受 accepted；其他强制 ID 才可按 GATE-001 对允许接受的 P1 使用有效 waiver，缺失、失败、跳过均阻断。
- 产生目标 `main` SHA 的 merged PR 已通过 `CI PR / required`；目标 SHA 本身存在成功的 `CI Main / verify`。
- `release_target` 合法，trusted baseline 可读取；新 identity 的 canonical version 严格大于 baseline，完全匹配的同一 identity recovery 例外。
- trusted baseline 是全部 `vX.Y.Z` tag 与 GitHub Release 的核心版本最大值，包含 prerelease、build metadata 和被标记无效的版本；`vX.Y.Z-*`/`vX.Y.Z+*` 与 `vX.Y.Z` 占用同一核心序号。collision 检查排除当前 identity 的完整匹配对象：reservation 自身，以及 policy/target/version/source SHA 完全匹配的 tag/release；其他任一已有 tag/release 大于或等于 target 时 fail closed，不允许重发同版本。
- draft reservation 的 version/source 与本次 target 一致；同 version 不同 source 必须冲突。
- 内核准入 `KERNEL-001` 至 `KERNEL-010` 通过。
- Xray 准入、协议矩阵、共享出站引用计数、连接复用、引擎崩溃重建通过。
- 指纹、DNS、WebRTC、IPv6、DoH、失败关闭测试通过。
- Docker 非 root、只读根、无 Docker socket、沙箱状态可确认；镜像从同一 source 独立构建两次且 digest 完全相等，比对结果作为 ACC-010 可复现构建证据。
- 管理 API、运行档案入站、CDP 和 noVNC 不暴露到非必要网络。
- 错误码、事件 schema、OpenAPI、数据库迁移与导出 schema 校验通过。
- GATE-001 机器可读报告通过：`TEST-PLAN.md` 的固定 required ID 集合（含 `FP-001..013`、全部 `RT/API/SEC`，排除 Tier B `PROXY-007` 与 `PROXY-030`）中所有项均为 `pass`，或仅对允许逐项接受的 P1 使用带完整 waiver 记录的 `accepted`；没有任何缺失、`fail`、`error`、`skipped` 或缺少 waiver 的 `accepted`。
- macOS 应用、嵌套 Chromium 内核及 renderer/GPU/utility helper 使用且只使用 Chromium JIT 的两项 entitlements；其他 helper 逐个 AdHoc 签名但不带 JIT；Xray-core/frameworks 不带 JIT。签名清单覆盖每个可执行文件，`codesign --verify --deep --strict`、`--options runtime`、`Signature=adhoc` 和角色级 entitlements 断言通过；真实页面加载通过，不要求公证或 Gatekeeper 成功。
- `SHA256SUMS`、SBOM、`LICENSE`、`NOTICE`、`LICENSES/`、更新清单、宿主侧 seccomp 与 `mien-chromium` AppArmor 发布物、artifact attestation 完整；attestation 覆盖 DMG、内核与 Xray-core 二进制、容器 digest、SBOM、seccomp 文件、AppArmor profile 与许可包，是必选门禁，没有“仓库计划不支持则跳过”的例外。
- `release` job 必须存在绑定当前 DMG SHA-256 的手动首次启动/升级通过记录；记录缺失、未通过或绑定旧 DMG 时拒绝公开。
- Docker 镜像扫描无未接受的高危/严重漏洞；镜像 digest 固定并签名。
- MPL-2.0、BSD-3-Clause 与容器内 GPL 组件许可复核通过。
- 项目自身许可证为 Apache-2.0；仓库根 `LICENSE` 与 `README` 链接就位（M4 门禁）。
- 发布说明列出所有已知限制与 Gatekeeper 手动放行步骤。

## 3. macOS 发布

1. 在 reservation 的 `source_sha` 上于 macOS arm64 runner 构建 `aarch64-apple-darwin`，运行完整 CI。
2. 下载并校验 fingerprint-chromium 148 DMG；解压内核；计算最终二进制 SHA-256。
3. 下载并校验 Xray-core v26.11.9 macOS ARM64；计算最终二进制 SHA-256。
4. 先对 Chromium 内核及其 renderer/GPU/utility helper、Xray 和 frameworks 做 AdHoc 签名，再签应用。Chromium 内核与每个 helper 使用 `codesign --force --sign - --timestamp=none --options runtime --entitlements mien-desktop/entitlements.plist`；Xray 与 frameworks 使用同一命令但不传 entitlements；不使用 `--deep` 代替逐个签名。
5. 仅为应用、Chromium 内核与 renderer/GPU/utility helper 传入 `mien-desktop/entitlements.plist`，只授予 Chromium V8 JIT 必需的两项；其他 helper、Xray 与 frameworks 不传 JIT entitlements。hardened runtime 下缺这两项会导致页面加载或 renderer 失败；不添加调试、网络、文件访问或其他放宽项。
6. 运行 `codesign --verify --deep --strict --verbose=2`；用 `codesign -dv --verbose=4` 断言 `Signature=adhoc` 和 `--options runtime`；按清单逐个断言应用、内核与 renderer/GPU/utility helper 只含上述两项，其他 helper/Xray/frameworks 不含 JIT，未分类文件失败；在干净机器上加载真实页面。`spctl --assess` 预期拒绝，不作为失败条件。
7. 生成 DMG、`SHA256SUMS`、SBOM、`LICENSE`/`NOTICE`/`LICENSES/`、来源信息、`entitlements.plist` 和 Tauri 更新清单；更新清单使用独立 minisign 密钥签名。
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
4. 启动时校验内核与 Xray SHA-256；失败拒绝启动。seccomp 与 AppArmor 发布物由部署者安装到宿主路径，容器以该宿主路径和 `apparmor=mien-chromium` 启动。
5. 健康检查 `/healthz`；就绪检查 `/readyz`。
6. 从同一 source_sha 使用两个独立干净 builder/缓存构建 linux/amd64；SOURCE_DATE_EPOCH 固定为 source commit 的提交时间（Unix 秒）并以 `--build-arg SOURCE_DATE_EPOCH=<epoch>` 传入。固定基础镜像、BuildKit 与 Dockerfile frontend digest、依赖锁和所有下载输入；image/oci exporter 设置 `rewrite-timestamp=true`（如 `--output type=oci,dest=<path>,rewrite-timestamp=true`），并设置 `--provenance=false`，不能把 rewrite-timestamp 当成 buildx 独立 CLI flag。导出格式、压缩参数和平台保持一致，比对导出产物中将发布的 OCI manifest/index digest；相等才无重构地发布第一次产物，校验 registry digest 不变，第二次仅留证据。签名、SBOM 与 attestation 在相等后单独生成；不相等视为不可复现缺陷，不接受或推 staging tag。
7. 生成 SBOM、漏洞扫描、容器许可扫描、cosign keyless 签名或固定 digest。
8. 在 x86_64 主机验证 Xvfb 有头、无头、伪有头、代理矩阵、容量、seccomp 宿主加载和数据卷。

## 5. GitHub Actions 发布流程

发布 workflow 为 `.github/workflows/release.yml`，完整设计见 [`CI.md`](CI.md)，版本与 identity 见 [`RELEASE-POLICY.md`](RELEASE-POLICY.md)：

resolve-target 读取 draft 并解析 source_sha，reserve 复核绑定后通过 workflow job outputs 将不可变 SHA 传给只读 test；其余需要 recovery 资产/identity 的 job 具备 draft 读取权限。所有源码 job checkout 该 SHA，不使用移动的 main HEAD。

1. `resolve-target` 读取 `release_target` 和 trusted baseline（全部 `vX.Y.Z` tag 和 release 的 SemVer 最大值，包含 prerelease 与已失效版本），先读取已有 reservation/identity，再执行 collision 检查；校验 `main` source 和 `CI Main / verify`；完全匹配的同一 identity 允许 recovery，其他任何 tag/release 大于或等于 target 时 fail closed。
2. `reserve` 创建或复用 draft GitHub Release，使用非 `v*` 的 reservation 标识作为 `tag_name`，写入 policy、target、version、source SHA 和 run ID；不创建 `vX.Y.Z`。
3. test 使用固定 source_sha 调用共享 CI，输出逐 ID 的机器可读分报告；不接触 release secrets 或 draft。
4. `macos-arm64` 与 `docker-amd64` 并行构建，分别上传 DMG、SBOM、校验和、宿主侧 seccomp 发布物与镜像 digest；`docker-amd64` 必须双构建且 digest 相等，第二次构建只做可复现性验证，不发布。
5. `stage-release` 把产物、seccomp/AppArmor 发布物与 `LICENSE`/`NOTICE`/`LICENSES/` 写入 draft，校验 SHA-256/digest，写入 `release-identity.json`；本次构建 digest 与 identity 已记录 digest 不一致时拒绝。
6. `attest` 为 DMG、内核与 Xray-core 二进制、容器 digest、SBOM、许可包、seccomp 文件和 AppArmor profile 生成 GitHub artifact attestation；没有降级或跳过分支。
7. attest 后由 gate-release 汇总测试、打包、attestation 与当前 DMG 手动测试，执行 GATE-001 并生成绑定 source/reservation/digests 的 release-gate.json；阶段 7 promote-docker 重验报告后才提升 digest，已存在 vX.Y.Z 只允许相同 identity/digest 的幂等复用，不得覆盖其他内容；绑定 release environment。
8. 阶段 8 release 再次验证完整 GATE-001、identity、promotion 与当前 DMG 的手动测试后才创建/验证 tag 并公开；单独重跑不跳过验证，绑定 release environment。
9. `verify-release` 下载公开资产，校验 checksums、SBOM、`LICENSE`/`NOTICE`/`LICENSES/`、seccomp/AppArmor 发布物、AdHoc 签名、entitlements、更新清单和 identity；失败时标记 release 无效。

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

- 项目许可已定为 Apache-2.0，仓库根 `LICENSE` 与 `README` 链接已就位；正式法律评审、`NOTICE` 与 `LICENSES/` 的实际构建仍未完成。
- `.github/workflows/` 的实际 YAML、branch protection、merge queue、`release` environment、draft reservation、仅允许 release App 绕过 creations 的 tag ruleset、Docker 双构建比对和强制 attestation 尚未落地。
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
