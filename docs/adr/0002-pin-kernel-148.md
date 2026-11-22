# ADR-0002：固定 fingerprint-chromium 148 并建立内核准入

- 状态：已接受
- 决策范围：浏览器内核版本、供应、校验和升级策略

## 背景

fingerprint-chromium 的版本差异直接影响指纹行为与稳定性。`150.0.7871.186` 提供 Windows 与 Linux 构建，但没有 macOS 构建；多个上游问题报告显示，150 在启用 canvas 伪装时会在 `getImageData` 或 `readPixels` 读回时触发渲染进程崩溃，导致 x.com 等站点无法加载，而 148 在相同配置下正常。`148.0.7778.215` 是当前同时提供 macOS ARM64、Linux x86_64 与 Windows x64 构建的最新版本，并且源码已按上游延迟策略公开。148 仍有已报告的 WebGL readPixels 越界访问问题，需要在运行环境中用沙箱和升级治理控制。

上游明确不提供技术支持，源码延迟发布，且产品不应维护一个完整的 Chromium 分支。因此需要在“使用最新版本”“保证 macOS 可用”“保证指纹稳定性”和“可审计性”之间做取舍。

## 决策

- 首个发布在所有平台固定使用 `148.0.7778.215`。
- macOS 使用 ARM64 DMG 中的内核；Docker 与后续 Windows 桌面使用对应平台的 148 构建。
- 内核二进制按平台随发布物供应，构建时下载并校验 SHA-256；不提交到 Git。
- 每个内核安装附带能力清单，声明支持的开关、取值、派生特征、已知缺陷和沙箱要求。
- 产品不修改、不分叉内核。升级只接受经过内核准入测试的上游版本。
- 应用更新与内核更新分离。已有档案默认继续使用原内核；运行中的档案不得被替换；用户显式迁移后重新校验指纹规格。
- 保留上一内核版本用于回滚，直到用户确认或达到保留期限。
- 内核准入必须覆盖：指纹探针一致性、canvas/WebGL 读回、目标站点加载、有头/无头、macOS/Docker、沙箱状态、崩溃与内存、代理泄漏。

## 备选方案

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| 使用 150 获取更新 Chromium | 安全补丁更新；Linux/Windows 可用 | 无 macOS；canvas 崩溃与站点失败；源码未公开 | 不采用 |
| 按平台混用 148 与 150 | 各平台取最新 | 指纹行为分叉；测试矩阵翻倍；导出迁移语义复杂 | 不采用 |
| 自建 148 补丁分支 | 可修复越界访问与已知缺陷 | Chromium 构建与签名成本高；需要长期安全跟进 | 暂不采用，作为安全兜底 |
| 固定 148 并准入升级 | 行为一致；macOS 可用；源码可审计 | 缺少 150 的安全修复；需沙箱缓解 | 采用 |

## 后果

- 产品必须把 Chromium 沙箱作为强制安全边界。Docker 默认拒绝无沙箱启动，只有显式授权才允许 `--no-sandbox`。
- 148 的已知越界访问报告进入风险登记；若上游长期不修复，启动自建补丁分支的预案。
- 能力清单成为 UI 与规格校验的唯一来源，避免暴露内核接受但无效果的参数。
- 升级节奏由产品准入决定，不跟随上游每次发布；安全公告需要单独评估。
- macOS Intel 不在支持范围，因为上游没有对应构建；Apple Silicon 是唯一 macOS 目标。
- Docker 镜像固定 `linux/amd64`，Apple Silicon 通过模拟运行；原生 ARM64 需要自行编译内核，不在首个发布范围。

## 证据

- 上游发行资产：148 提供 macOS DMG、Linux TAR.XZ、Windows ZIP；150 没有 macOS 构建。
- 上游 issue #94、#95、#104、#112：150 在 canvas 读回时崩溃；148 正常。
- 上游 issue #102：148 的 WebGL readPixels 对打包 16 位格式存在越界访问报告。
- 上游 issue #108：148 Windows 上 CreepJS 可能卡住，测试需使用自有探针为主。
- 上游 README：源码延迟发布，项目不提供技术支持。

## 参考

- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- [`../specs/fingerprint-engine/SPEC.md`](../specs/fingerprint-engine/SPEC.md)
- [`../specs/browser-runtime/SPEC.md`](../specs/browser-runtime/SPEC.md)
- <https://github.com/adryfish/fingerprint-chromium/releases/tag/148.0.7778.215>
