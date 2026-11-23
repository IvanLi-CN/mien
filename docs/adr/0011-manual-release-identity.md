# ADR-0011: 手动 release target 与不可变发布 identity

## 状态

已接受。

## 背景

Mien 同时发布 macOS DMG 和 Docker 镜像，未来还需要让多个消费者复用同一版本。仅靠维护者推送 `vX.Y.Z` tag 触发 workflow 有三个问题：

- tag 本身已经包含 exact version，但没有记录 owner 的 target 选择，也没有 trusted baseline 解析。
- 若构建失败后重建，没有 durable reservation 保证同一次发布仍使用同一 source 和 artifact digest。
- tag push 无法区分“新发布”和“同一 identity 的 recovery”，容易在重跑时产生 successor 或改写 tag。

项目需要把“owner 选择发布目标”和“workflow 解析并固化 identity”分开，同时继续使用 GitHub Actions 和 AdHoc 分发。

## 决策

- 发布只由 `workflow_dispatch` 的单个 `release_target` 输入触发；支持 `patch`、`minor`、`major` 和 explicit `X.Y.Z`。不提供默认值，不从 PR label、合并顺序或兼容性分类推导 target。
- `Version Policy` 为 `mien-semver-v1`：SemVer stable，trusted baseline 来自最新公开、非 draft、非 prerelease GitHub Release；首个 release 只允许 explicit `0.1.0`。
- workflow 在 reservation 前解析 canonical version，并绑定 `policy_id`、`release_target`、`canonical_version`、`source_sha`、`reservation_id`、artifact digest 和 publication target。
- 在构建前创建 GitHub Release draft 作为 durable reservation；draft 使用 `vX.Y.Z` 作为 tag name，但不创建公开 tag。同 version、同 source 复用；同 version、不同 source 冲突。
- `CI Main / verify` 和 `CI PR / required` 是发布前置条件；release source 必须是 `main` 上的正常 PR 合并结果。release workflow 不直接写 `main`。
- Release 是独立阶段：draft 只接收已验证产物；所有 digest、checksum、SBOM、AdHoc 签名和更新清单通过后，才创建或验证 tag 并公开 GitHub Release。
- tag 只在 Release 阶段创建或验证；已存在 tag 时必须验证 peeled commit，指向不同 source 则停止，不改写。
- 失败重跑使用相同 `release_target` 和相同 `source_sha`，读取已有 reservation，补齐缺失步骤；不重新计算 baseline，不创建 successor，不改变版本或 channel。
- 本仓库的 Release 后不再自动 Deploy；后续部署消费者必须复用 `release-identity.json` 的 digest，不能按分支或 `latest` 重新构建。
- 每次发布至少构建一次 macOS DMG 和 Docker 镜像；两者可以共享 canonical version，但各自验证 digest。

## 备选方案

| 方案 | 优点 | 代价 | 结论 |
| --- | --- | --- | --- |
| tag push 触发 | 简单，tag 即 identity 的一部分 | 无 owner target、无 baseline 解析、无 durable reservation；重跑容易产生歧义 | 不采用 |
| PR label 触发 | 与 PR 流程结合 | 本项目不需要 label-driven release；label 与手动输入会形成双真相源 | 不采用 |
| 自动计算下一版 | 少一次人工输入 | 违反 owner 选择 release target 的边界，merge 顺序或重跑会改变版本 | 不采用 |
| 单个 `release_target` + draft reservation | target 明确；version、source、digest 可重放；recovery 可验证 | 需要额外 reservation 和 identity 合同 | 采用 |

## 后果

- 维护者必须先给 target，再等待 workflow 解析；没有默认 `patch`。
- 发布期间会先存在一个维护者可见的 draft release；它不是公开版本，但删除它会破坏 reservation，必须由发布流程处理。
- tag 创建被推迟到 Release 阶段，降低构建失败留下错误 tag 的概率。
- 失败重跑必须复用同 identity；修复 source 需要新的 release boundary，不能伪装成 recovery。
- `release-identity.json` 和 draft 正文成为排障、审计和下游 digest 消费的入口。
- 需要一个可执行的 policy validator 和 reservation 检查；在当前文档阶段，这些是 M4 前的实现前置条件。

## 验证

- `release_target` 缺失或非法时 workflow 拒绝。
- 无 baseline 时只有 explicit `0.1.0` 通过；symbolic target 失败。
- 同 version、不同 source 的 reservation 冲突。
- 已存在 tag 指向不同 source 时停止。
- 失败重跑使用同 target/source 时返回同 canonical version 和同 digest。
- `CI Main / verify` 缺失时 release 不开始构建。
- 公开 release 的 `verify-release` 失败时标记无效，不静默成功。

## 参考

- [`../RELEASE-POLICY.md`](../RELEASE-POLICY.md)
- [`../CI.md`](../CI.md)
- [`../RELEASE.md`](../RELEASE.md)
- [`0010-adhoc-macos-distribution.md`](0010-adhoc-macos-distribution.md)
- [SemVer 2.0.0](https://semver.org/)
- [GitHub Releases API](https://docs.github.com/en/rest/releases/releases)
