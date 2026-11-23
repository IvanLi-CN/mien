# ADR-0010: macOS AdHoc 分发与 GitHub Actions 发布

## 状态

已接受。

## 背景

Mien 首个发布面向 macOS Apple Silicon。常规 macOS 分发需要 Developer ID 证书、Hardened Runtime、公证和 `stapler`，这要求 Apple Developer 账号、证书管理和公证凭据。项目决定不建立这条签名链路，只使用 AdHoc 签名。

这改变了分发的信任模型：AdHoc 签名可以检测应用包在签名后的非预期修改，但不提供 Apple 可验证的发布者身份，也不能通过 Gatekeeper 的首次启动检查。发布流程必须明确这个限制，不能把 AdHoc 描述成 Developer ID 或公证。

## 决策

- macOS 应用、嵌套的 fingerprint-chromium、Xray-core 和 frameworks 使用 AdHoc 签名，并启用 hardened runtime。JIT entitlements 只传给应用与嵌套 Chromium 内核：`codesign --force --sign - --timestamp=none --options runtime --entitlements mien-desktop/entitlements.plist`；Xray-core 与 frameworks 使用同一 AdHoc 签名但不带 JIT 权限。
- 先逐个签嵌套二进制，再签应用；用 `codesign --verify --deep --strict --verbose=2` 验证结构完整性，用 `codesign -dv --verbose=4` 断言 `Signature=adhoc`。
- 不执行 `notarytool`、`stapler` 或 Gatekeeper 成功验证。`spctl --assess` 对 AdHoc 应用预期失败，CI 只记录这个预期结果。
- 发布物提供 `SHA256SUMS`、SBOM、许可证、已知限制和 Tauri 更新清单，并必须为 DMG 和容器 digest 生成 GitHub artifact attestation。
- macOS 更新使用 Tauri 的独立更新签名密钥，和 Apple 代码签名无关；客户端拒绝未签名或降级的更新包。
- 发布由 GitHub Actions 的 `release.yml` 执行，由手动 `release_target` 触发；tag 只在 Release 阶段创建或验证，`release` environment 需要人工批准。target、reservation 和 recovery 见 [`ADR-0011`](0011-manual-release-identity.md)。
- 不在应用、安装器或发布脚本中自动移除 `com.apple.quarantine`；发布说明要求用户先校验 SHA-256 或 attestation，再手动放行。
- 不保存 Apple 证书、Apple ID、Team ID 或 App-Specific Password 作为 CI secret。

## 备选方案

| 方案 | 优点 | 代价 | 结论 |
| --- | --- | --- | --- |
| Developer ID + 公证 | Gatekeeper 正常通过，用户无额外步骤，发布者身份可验证 | 需要 Apple 账号、证书、公证凭据、持续维护和发布等待 | 不采用；项目明确不使用 |
| 无签名 | 打包最简单 | 没有任何代码完整性信号，仍会被 Gatekeeper 阻止 | 不采用 |
| 自签名证书 | 可做本地验证 | 不进入 Apple 信任链，用户仍需手动放行，证书分发和轮换增加复杂度 | 不采用 |
| AdHoc 签名 + 校验和/attestation | 不需要 Apple 证书；可检测包被改动；GitHub Release 可追溯 | Gatekeeper 首次启动被阻止；没有发布者身份；需要清楚的手动放行说明 | 采用 |

## 后果

- 首次启动在干净 macOS 机器上必然被 Gatekeeper 阻止；这是预期行为，不是构建失败。
- 用户必须从可信 GitHub Release 获取资产，先校验 SHA-256 或 attestation，再手动放行。仅依赖同一渠道的校验和不能抵御发布账户被攻破。
- 没有 Apple 公证，无法向用户提供“已验证发布者”的系统级证明；安全文档和 UI 必须如实披露。
- 自动更新仍可工作，但信任来自 Tauri 更新签名，而不是 Apple 签名；更新密钥成为关键发布资产。
- GitHub Actions 的 `release` environment、tag 保护、artifact attestation 和发布后校验成为主要发布控制。
- 发布说明必须包含 Gatekeeper 处理步骤，禁止建议全局关闭 Gatekeeper。
- 应用与 Chromium 内核的签名命令必须传入 JIT entitlements；遗漏时 `codesign --verify` 仍会通过，但 hardened runtime 下内核启动失败，因此 entitlements 断言独立于结构校验。Xray-core 与 frameworks 不得复制 JIT 权限。

## 验证

- `codesign --verify --deep --strict --verbose=2` 对应用、内核和 Xray 通过。
- `codesign -dv --verbose=4` 显示 `Signature=adhoc`，且不存在 Developer ID TeamIdentifier。
- `codesign -d --entitlements :-` 对应用与 Chromium 内核只显示 `com.apple.security.cs.allow-jit` 与 `com.apple.security.cs.allow-unsigned-executable-memory`，Xray-core 与 frameworks 不显示 JIT 权限；签名后的内核可在干净机器启动。
- `spctl --assess --type execute` 预期拒绝；CI 记录结果但不把拒绝当作失败。
- `SHA256SUMS` 可校验所有发布资产；Tauri 更新清单签名可验证，并拒绝降级。
- 在干净 macOS 13+ 机器按发布说明完成首次启动和更新测试。
- 分支保护要求 `CI PR / required`；release workflow 在 `release` environment 审批后才公开资产。

## 参考

- [`../CI.md`](../CI.md)
- [`../RELEASE.md`](../RELEASE.md)
- [`../SECURITY.md`](../SECURITY.md)
- [`../TEST-PLAN.md`](../TEST-PLAN.md)
- [Apple Gatekeeper 与公证](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
- [Tauri updater 签名](https://v2.tauri.app/plugin/updater/)
