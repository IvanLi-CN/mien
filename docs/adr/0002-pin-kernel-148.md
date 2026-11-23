# ADR-0002：固定 fingerprint-chromium 148 并建立内核准入

- 状态：已接受
- 决策范围：浏览器内核版本、供应、校验和升级策略

## 背景

fingerprint-chromium 的版本差异直接影响指纹行为与稳定性。`150.0.7871.186` 提供 Windows 与 Linux 构建，但没有 macOS 构建；多个上游问题报告显示，150 在启用 canvas 伪装时会在 `getImageData` 或 `readPixels` 读回时触发渲染进程崩溃，导致 x.com 等站点无法加载，而 148 在相同配置下正常。`148.0.7778.215` 是当前同时提供 macOS ARM64、Linux x86_64 与 Windows x64 构建的最新版本，并且源码已按上游延迟策略公开。148 仍有已报告的 WebGL readPixels 越界访问问题，需要在运行环境中用沙箱和升级治理控制。

上游明确不提供技术支持，源码延迟发布，且产品不应维护一个完整的 Chromium 分支。因此需要在“使用最新版本”“保证 macOS 可用”“保证指纹稳定性”和“可审计性”之间做取舍。

## 决策

- 首个发布在所有平台固定使用 `148.0.7778.215`。
- macOS 使用 ARM64 DMG 中的内核；Docker 与后续 Windows 桌面使用对应平台的 148 构建。
- 内核二进制按平台随发布物供应，构建时下载并校验发布资产摘要；不提交到 Git。
- 发布资产摘要与安装后可执行文件摘要分开记录：清单与发布物中的 `sha256` 是发布资产（DMG/TAR.XZ/ZIP）摘要；安装记录同时保存 `asset_sha256`（下载资产）和 `binary_sha256`（解包后可执行文件）。运行时以安装记录的 `binary_sha256` 校验可执行文件，不得用清单的发布资产摘要代替。
- 可执行文件缺失返回 `KERNEL_NOT_FOUND`，摘要不符返回 `KERNEL_CHECKSUM_MISMATCH`；两种情况都拒绝启动，且不启动协议引擎。
- 每个内核安装附带能力清单，声明支持的开关、取值、派生特征、已知缺陷和沙箱要求。
- 能力清单是可演进契约：`flags` 在所有平台声明 `window-size`（类型 `string`）；沙箱要求用 `sandbox_kind`（`namespace|seatbelt|appcontainer|none`）表达，取代 `requires_namespace_sandbox` 布尔；`network_controls` 允许 `disabled|unsupported` 与 `block|unsupported`，使缺失该能力的平台清单仍是合法 fixture；`flagSpec` 提供 `per_platform_values`，CPU、语言、时区等能力校验由清单数据驱动，而不是硬编码在 UI 或校验器中。
- 每个 148 平台清单必须登记上游已知问题 #98、#99（canvas 子区域读回与完整读回不一致），并保留已登记的 148 问题 #102、#108；150 的 #94、#95、#104、#112 只作为版本选择证据，不写入 148 清单。
- 产品不修改、不分叉内核。升级只接受经过内核准入测试的上游版本。
- 应用更新与内核更新分离。档案固定自己的 `kernel_version`，同一宿主可并存多个内核版本；应用更新不静默改写档案内核，运行中的档案不得被替换。
- 内核迁移是显式的每档案操作：迁移前预检平台、能力清单、指纹规格（含窗口尺寸与派生特征）和沙箱可用性；迁移写入审计事件；迁移后重新校验指纹规格并提示派生特征变化。运行中的档案必须先停止才能迁移。
- 保留上一内核版本用于回滚，直到用户确认或达到保留期限。
- 内核准入必须覆盖：指纹探针一致性、canvas/WebGL 读回、目标站点加载、有头/无头、macOS/Docker、沙箱状态、崩溃与内存、代理泄漏。
- Docker 的 namespace 沙箱必须配合强制 seccomp profile；profile 路径与允许列表是 M0 交付物和发布工件，M0 门禁必须在 Docker 中以非特权 user namespace 证明其生效。不得把 seccomp 描述为“按需”加固。

## 备选方案

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| 使用 150 获取更新 Chromium | 安全补丁更新；Linux/Windows 可用 | 无 macOS；canvas 崩溃与站点失败；源码未公开 | 不采用 |
| 按平台混用 148 与 150 | 各平台取最新 | 指纹行为分叉；测试矩阵翻倍；导出迁移语义复杂 | 不采用 |
| 自建 148 补丁分支 | 可修复越界访问与已知缺陷 | Chromium 构建与签名成本高；需要长期安全跟进 | 暂不采用，作为安全兜底 |
| 固定 148 并准入升级 | 行为一致；macOS 可用；源码可审计 | 缺少 150 的安全修复；需沙箱缓解 | 采用 |

## 后果

- 产品必须把 Chromium 沙箱作为强制安全边界。Docker 默认拒绝无沙箱启动，只有显式授权才允许 `--no-sandbox`。
- 安装记录区分发布资产摘要与可执行文件摘要，重新打包、镜像层或解包差异不会因为资产摘要相同而静默通过；运行时校验只认 `binary_sha256`。
- 档案固定内核版本意味着宿主上会长期存在混合版本。每个已安装版本必须各自带有匹配的能力清单、`sandbox_kind` 与 seccomp 配置；混合版本下的启动、迁移和回滚都必须纳入测试。
- 迁移与回滚是显式操作，失败时保留原内核与档案绑定，不产生“部分迁移”的档案；审计事件必须记录前后版本、预检结果和执行者。
- 148 的已知越界访问报告进入风险登记；若上游长期不修复，启动自建补丁分支的预案。
- 能力清单成为 UI 与规格校验的唯一来源，避免暴露内核接受但无效果的参数。
- 升级节奏由产品准入决定，不跟随上游每次发布；安全公告需要单独评估。
- macOS Intel 不在支持范围，因为上游没有对应构建；Apple Silicon 是唯一 macOS 目标。
- Docker 镜像固定 `linux/amd64`，Apple Silicon 通过模拟运行；原生 ARM64 需要自行编译内核，不在首个发布范围。

## 证据

- 上游发行资产：148 提供 macOS DMG、Linux TAR.XZ、Windows ZIP；150 没有 macOS 构建。
- 上游 issue #94、#95、#104、#112：150 在 canvas 读回时崩溃；148 正常。
- 上游 issue #102：148 的 WebGL readPixels 对打包 16 位格式存在越界访问报告。
- 上游 issue #98、#99：148 的 canvas 子区域读回与完整读回可能不一致；清单与测试报告必须显式登记这一限制。
- 上游 issue #108：148 Windows 上 CreepJS 可能卡住，测试需使用自有探针为主。
- 上游 README：源码延迟发布，项目不提供技术支持。

## 参考

- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- [`../specs/fingerprint-engine/SPEC.md`](../specs/fingerprint-engine/SPEC.md)
- [`../specs/browser-runtime/SPEC.md`](../specs/browser-runtime/SPEC.md)
- <https://github.com/adryfish/fingerprint-chromium/releases/tag/148.0.7778.215>
