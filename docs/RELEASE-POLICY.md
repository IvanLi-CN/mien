# Mien 手动版本发布策略

本文定义 `release_target`、版本解析、trusted baseline、release identity、reservation、发布映射、并发和 recovery。它对应 `manual-version-release-delivery` 的设计边界，是 [`CI.md`](CI.md) 与 [`RELEASE.md`](RELEASE.md) 的共同输入。

## 1. 决策边界

- 发布是维护者显式选择 target 的手动操作，不由 PR 合并、main push、tag push 或 PR label 自动触发。
- 使用一个 `release_target` 字段表达目标；Symbolic 与 explicit 是执行端 policy 的两个解析分支，不增加 `mode`、`impact` 或第二个版本字段。
- 版本解析、baseline 读取、reservation、artifact digest 和 publication 都由 workflow 执行；Agent 只传递 owner 的 target，不自行计算或替换 canonical version。
- 版本策略是 `mien-semver-v1`。以后改变 grammar、baseline 或 publication mapping 必须新增 policy revision，不静默改变旧 identity 的语义。
- Release 与 Deploy 分离。本仓库负责构建、签名、校验和 GitHub Release；不在本仓库内自动部署到任何服务器。后续部署必须复用 Release 的 digest，不能按 `latest` 或分支重新构建。

## 2. `release_target`

`workflow_dispatch` 只接受一个必填输入 `release_target`，没有默认值：

| 输入 | 解析路径 | 示例 |
| --- | --- | --- |
| `patch` | 对 trusted baseline 递增 patch | `0.1.0` → `0.1.1` |
| `minor` | 对 trusted baseline 递增 minor | `0.1.1` → `0.2.0` |
| `major` | 对 trusted baseline 递增 major | `0.2.0` → `1.0.0` |
| `X.Y.Z` | explicit version；可接受 `vX.Y.Z` 输入，但 canonical 不带 `v` | `0.2.0` |

约束：

- 首个 release 没有 trusted baseline，只允许 explicit `0.1.0`；`patch`、`minor`、`major` 在 baseline 缺失时必须 fail closed。
- v1 只发布 stable SemVer；`alpha`、`beta`、`rc` 和 build metadata 不进入 v1 policy。需要 prerelease 时必须先修订 policy。
- 缺少 target、target 无法解析、target 小于或等于 baseline、explicit target 已被其他 source 占用，都拒绝发布。
- 不提供 `patch` 默认值，不从 PR 数量、diff、label、兼容性分类或 `VERSION` 文件推导 target。
- 本项目是 target-only manual release；PR label、per-PR impact record 和兼容性分类不参与 target 选择或 release eligibility。
- 不把 owner 的 symbolic target 提前替换成 Agent 计算的 exact version；canonical version 由 workflow 在 reservation 前解析并记录。

## 3. Trusted baseline

Trusted baseline 是仓库中**所有已存在的 `vX.Y.Z` Git tag 与 GitHub Release 的核心版本最大值**，不论其 draft、prerelease、invalidated 或公开状态；所有已存在版本均占用版本序号。

- 不使用 `VERSION` 文件、main 提交数、PR 合并顺序或 workflow artifact 作为版本真相；workflow 从 GitHub 读取所有 `vX.Y.Z` tags 与 Releases，任何已存在版本都占用序号。
- 读取 baseline 时按 SemVer 排序，不按创建时间或字符串排序；历史 prerelease/build metadata 的版本按 `MAJOR.MINOR.PATCH` 核心元组占用同一序号，`v1.0.0-rc.1` 与 `v1.0.0+meta` 都会阻止新的 `1.0.0`。
- 没有 baseline 时只允许 explicit `0.1.0`；其他输入拒绝。
- baseline 在 target 解析、reservation、Release 前分别重新读取；权限、API 或身份不可确认时 fail closed。
- prerelease、draft 或 invalidated Release 对应的 tag/version 仍占用序号；任何已存在 tag/release 只有在完整 identity 与 reservation 匹配时才允许 recovery。

## 4. Ordering 与 collision

- 使用 SemVer 2.0.0 的 stable 比较规则；canonical version 是 `MAJOR.MINOR.PATCH`。
- 新 identity 的 target 必须严格大于所有已存在 tag/release 的核心版本；任一已存在版本大于或等于 target 都拒绝。相同 identity 的 recovery 是唯一允许复用已存在版本的情形：先读取 reservation 并验证 policy/target/version/source SHA 完全一致，再从 collision 集合中排除该 identity 的完整匹配对象——reservation 自身，以及 policy/target/version/source SHA 完全匹配的 tag/release——然后放行；其他 identity 使用同一版本仍然冲突。不得仅因 version 相同就复用。
- 相同 target 只允许作为同一 source identity 的 idempotent retry/recovery，不得重新分配 successor。
- tag `vX.Y.Z` 已存在时，只能读取并验证其 peeled commit；如果指向不同 source，停止，不改写 tag。
- Release record、container tag 或 GitHub Release 已属于另一个 source identity 时，停止并报告冲突。

## 5. Release identity

一次 release 的 identity 至少包含：

| 字段 | 含义 |
| --- | --- |
| `policy_id` | `mien-semver-v1` |
| `release_target` | owner 输入的原始值，例如 `minor` 或 `0.2.0` |
| `canonical_version` | workflow 解析出的 `X.Y.Z` |
| `source_sha` | 新 identity 首次 dispatch 时从 `main` 固定的 SHA；所有 recovery 从 reservation 读取并 checkout 该 SHA |
| `reservation_id` | GitHub Release draft 的 release ID |
| `artifacts` | macOS DMG、Docker image 等产物的 SHA-256 或 digest |
| `publication` | tag `vX.Y.Z`、GitHub Release、GHCR image tag/digest |

identity 写入 draft release 正文和 `release-identity.json`；相同 tuple 的 retry 必须返回同一 canonical version 和同一组已验证 digest。

## 6. Reservation

在构建、tag 创建或公开 Release 之前，workflow 使用 GitHub Release draft 建立 durable reservation：

- draft 的 `tag_name` 使用非 `v*` 的 reservation 标识 `mien-reservation-<canonical>-<run_id>`，`target_commitish` 为 `source_sha`；reservation 标识不占用 SemVer baseline，也不作为公开版本。Release 阶段由 release GitHub App 创建或验证 `vX.Y.Z`，再把 draft 的 `tag_name` 更新为 `vX.Y.Z`（或由 release job 建立最终 release）。
- draft 正文记录 policy、target、canonical version、source SHA、workflow run 和 reservation 状态。
- 已存在同 version、同 source 的 draft 时复用；同 version、不同 source 时冲突；任一 tag/release 占用该版本时，只能验证并恢复完全相同的已记录 identity。
- reservation 是版本和 source 的 claim；后续 job 不从旧的 workflow artifact 或本地状态重新分配版本。
- 任何需要 tag、公开 Release 或不可逆上传的步骤都必须在 reservation 成功之后执行。
- `stage-release` 成功后才把 artifact digest 写入 identity；已记录 digest 的 retry 必须复用同一字节和 digest，不重新构建或替换资产。只有尚未完成 stage 的失败才允许在相同 reservation 下重新构建。

## 7. 并发

- release workflow 使用固定 `release` concurrency group，`cancel-in-progress: false`。
- 同一时间只允许一个 release evaluation；同一 group 只保留一个 pending run，更新的 dispatch 会取消先前 pending 的 run，维护者必须确认自己的 run 仍在排队，不能假定会自动等待。
- PR、main 和 release 使用独立 namespace；PR 的取消策略不影响 release。
- 重跑必须复用已绑定的 identity，不创建并行 successor。

## 8. Source 与质量门禁

- 新 identity 的 `workflow_dispatch` 必须从 `main` 发起并固定 source SHA；同 identity recovery 以 reservation 中的 source SHA 为准，即使 `main` 已前进。
- source 必须是正常 PR 合并结果；release workflow 不直接写 `main`，不绕过 review 或 required checks。
- 目标 SHA 必须存在成功的 `CI Main / verify` 结论；缺失时 fail closed。
- PR gate 证明候选可合并，`CI Main / verify` 证明 mainline 可发布；两者不能互相替代。
- release 前重新读取 branch protection、check run、tag 和 release 状态；不能只依赖 dispatch 时的 UI 绿色状态。

## 9. Publication mapping

- Git tag：`vX.Y.Z`，只在 Release 阶段创建或验证；不覆盖已存在 tag。
- GitHub Release：标题和 tag 使用 canonical version；附 DMG、`SHA256SUMS`、SBOM、`LICENSE`、`NOTICE`、`LICENSES/`、`release-identity.json` 和 Tauri 更新清单。
- Docker：`ghcr.io/<owner>/mien-server:vX.Y.Z` 和固定 digest；不使用可变 `latest` 作为生产发布映射。
- Docker 构建先推 `staging-<run_id>` 临时 tag；验证通过后在 `promote-docker` 阶段按 digest 提升为 `vX.Y.Z`，不重复构建。
- macOS 使用 AdHoc 签名，不公证；Gatekeeper 手动放行说明随 Release 一起发布。Apple 签名不是 publication identity。
- 每种发布产物只构建一次；Docker 为可复现性验证进行的第二次构建不发布、不写 identity，Release 和后续部署消费者复用第一次构建的同一 digest，不重新构建或改写版本。

## 10. Release 与 Deploy 阶段

1. `resolve-target`：读取 target 和 baseline，解析 canonical version，读取 source 和 `CI Main / verify`。
2. `reserve`：创建或复用 draft release；写入 identity。
3. `test`：运行完整 CI 和 e2e；失败不构建发布物。
4. `macos-arm64`、`docker-amd64`：在 reservation 的 `source_sha` 上构建并记录 digest，不公开发布。
5. `stage-release`：把产物、SBOM、`LICENSE`/`NOTICE`/`LICENSES/`、校验和与 identity 写入 draft；校验 digest。
6. `attest`：为 DMG、内核与 Xray-core 二进制、容器 digest、SBOM、许可包与 seccomp 发布物生成并验证 artifact attestation；缺失或失败即停止，不存在跳过分支。
7. `promote-docker`：将已验证 digest 提升到 `vX.Y.Z`；绑定 `release` environment。
8. `release`：在 Docker 提升成功后校验当前 DMG 的手动测试通过记录，再创建或验证 tag，公开 GitHub Release；绑定 `release` environment。
9. `verify-release`：下载公开资产，校验 checksum、AdHoc 签名、更新清单和 identity；失败标记 release 无效。

Deploy 不在本仓库自动执行。任何部署目标都必须消费 `release-identity.json` 中已验证的 artifact digest，并保持 Release 的 canonical version、source SHA 和 digest 不变。

## 11. Recovery

- 失败后以相同 `release_target` 重跑；先读取已有 reservation/identity，并 checkout reservation 中绑定的 `source_sha`，即使 `main` 已前进。不得从本次 dispatch 的新 `main` SHA 重建旧 identity；若原 SHA 无法 checkout，则停止并报告。
- 同 target、同 source 只补齐缺失步骤；不重写 tag、不改变 channel、不创建新版本。
- 已记录 artifact digest 时，对应构建 job 必须跳过并复用已上传资产；`stage-release` 必须拒绝与 identity digest 不同的产物。公开 release 存在时只做验证，不重新构建或覆盖资产；若重新构建 DMG，先前手动测试证据失效并须重测。
- 已存在 tag 时，读取 tag 的 peeled commit 并与 `source_sha` 比较；只有 identity 成立才恢复。
- 已绑定 identity 后，main 前进不改变恢复 source；只有 owner 主动发起全新发布时，才使用新的 main SHA 和新的 `release_target` 建立新 identity。
- recovery 不降低 `CI Main / verify`、artifact checksum、Release 或 `verify-release` 门禁。
- 如果旧 source 没有合法 identity，停止并请求 owner 选择新的 release boundary。
- 删除 draft reservation 被视为放弃该 identity，必须由 owner 明确决定；workflow 不静默清理 draft。

## 12. 失败关闭条件

以下任一条件不满足时停止：

- `release_target` 缺失、非法、无 baseline 或无法解析；或历史 prerelease/build metadata 的核心版本仍占用同一序号。
- 任一已存在 tag/release 版本大于或等于新 target（同 identity recovery 除外），或目标与已有 identity 冲突。
- source 不是 `main`、不是 PR 合并结果，或 `CI Main / verify` 未成功。
- reservation 的 version/source 与请求不一致。
- tag 已存在且指向不同 SHA。
- 任一产物 digest、SBOM、`LICENSE`/`NOTICE`/`LICENSES/`、签名、checksum、更新清单或必需 attestation 缺失/不匹配。
- 发布环境未批准、权限不足或 release secret 不可用。
- 公开 release 已属于另一个 identity。

## 13. Owner 前置条件

- 确认 release target、trusted baseline 来源和首个 `0.1.0` release。
- 配置 main branch protection、`CI PR / required`、`CI Main / verify` 和 `release` environment 审批；`stage-release`、`promote-docker`、`release` 的独立 re-run 都必须重新进入审批。
- 配置 repository ruleset：目标 `v*`，启用 `Restrict updates`、`Restrict deletions` 与 `Restrict creations`；bypass 仅允许 release GitHub App（由受保护的 `release` environment 签发），不允许任何人类角色或日常 `GITHUB_TOKEN` 绕过。release job 通过该 App 创建新 tag；规则不得允许改写或删除既有 tag，也不得让任意有 tag 写权限的身份抬高 trusted baseline。
- 配置 Tauri 更新签名密钥；不配置 Apple Developer ID 或公证凭据。
- 配置 GitHub Release、GHCR 的权限和 artifact retention。
- 决定 Release 后的部署目标；部署授权、凭据和 smoke test 不属于本仓库的 release workflow。

## 14. 参考

- [`CI.md`](CI.md)
- [`RELEASE.md`](RELEASE.md)
- [`adr/0011-manual-release-identity.md`](adr/0011-manual-release-identity.md)
- [`adr/0010-adhoc-macos-distribution.md`](adr/0010-adhoc-macos-distribution.md)
- [SemVer 2.0.0](https://semver.org/)
- [GitHub Releases API](https://docs.github.com/en/rest/releases/releases)
