# 指纹规格与启动参数

> 本文件是指纹主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: 产品需要把用户可编辑的浏览器画像转换为 fingerprint-chromium 能够理解、且内部一致的启动参数，并保证同一规格可复现。
- In scope: 指纹规格 schema、种子、显式字段、派生特征、一致性规则、能力清单校验、启动参数构建、规格迁移、限制呈现。
- Out of scope: 档案 CRUD 与数据目录（由 `profile-management` 负责）、内核进程监督（由 `browser-runtime` 负责）、代理连接与 DNS（由 `proxy-gateway` 负责）。

## Terms and Interfaces

- `fingerprint spec`: 带 `schema_version` 的结构化规格，包含显式字段、种子、`window`、可选 `spoof`、`webrtc` 与 `network`。
- `explicit field`: 用户可编辑并通过内核开关表达的字段，例如平台、品牌、语言、时区、CPU 核心数。
- `derived trait`: 由种子和内核版本决定、不逐项编辑的特征，例如 canvas 噪声、音频噪声、WebGL GPU 参数、设备内存。
- `host-bound field`: 受宿主显示、字体或图形栈影响、产品不能可靠伪造的特征，例如 `screen.width`、`devicePixelRatio`、字体枚举、`navigator.plugins`。
- `window`: 指纹规格内的窗口/视口尺寸 `{width, height}`，是指纹规格的规范字段和窗口尺寸的唯一权威来源；它不是屏幕分辨率，也不出现在档案顶层字段、启动偏好、导出 `runtime` 或 `window_json` 列中。
- `capability manifest`: 内核版本与平台组合实际支持的能力清单，包含 `flags`（含必填 `window-size`，类型 `string` 与候选集合）、`sandbox_kind`、`network_controls` 与 `flagSpec.values`/`per_platform_values`。
- `install record`: 内核安装记录，同时保存发布资产摘要 `asset_sha256` 与解包后可执行文件摘要 `binary_sha256`；清单 `sha256` 只是发布资产摘要。
- `sandbox_kind`: 内核构件依赖的沙箱机制，取值 `namespace|seatbelt|win32-restricted-token|none`；声明沙箱种类不代替启动前的有效沙箱校验。
- `isolation verdict`: 启动校验后的隔离结论，取值 `enforced|unsandboxed|unknown`；由运行时写入 `runtimes.isolation` 并驱动 proof strip。
- Interface: `POST /api/v1/profiles` 与 `PATCH /api/v1/profiles/{id}` 中使用的指纹 JSON；核心函数 `validate_spec`、`build_launch_argv`；内核能力清单 JSON；内核安装记录。

## Requirements

### REQ-FP-001

- The system MUST define a versioned stored/exported fingerprint spec schema containing `schema_version`, `seed`, `platform`, `platform_version`, `brand`, `brand_version`, `hardware_concurrency`, `languages`, `timezone`, `window`, `spoof`, `webrtc`, and `network` fields. Create and update requests use the companion input schema, which omits `seed`; the server generates a fresh seed on create/duplicate and never changes it on update. `window` is the canonical window-size location: it MUST NOT be duplicated in profile payloads, runtime preferences, exports, or a `window_json` column, and it maps to the manifest `window-size` flag (type `string`, `WIDTH,HEIGHT`).
- Optionality: `spoof` and its categories are optional and default to enabled; omitted values MUST NOT be normalized into a different meaning. Required fields and types otherwise follow the schema.
- Inputs: 用户编辑、预设、导入文件或复制操作；创建/更新请求不携带 `seed`，导入使用导出规格并携带 `seed`。
- Outputs: 通过 schema 校验的规格；未知字段、缺失必填字段、错误类型和越界值必须被拒绝；窗口尺寸只能从指纹规格读取，不创建档案顶层 `window` 字段或 `window_json` 存储；更新请求试图修改 `seed` 时拒绝。

### REQ-FP-002

- The system MUST generate a non-zero 32-bit seed using a cryptographically secure random source; every create and every duplicate copy (`copy_fingerprint=true`) MUST generate a fresh seed rather than reusing the source profile's seed. If a freshly generated seed equals any existing profile's seed in any status, the generator MUST draw again before persisting; the retry is silent and never surfaces a conflict error. Import preserves the exported seed for reproducibility, so an imported profile may share a seed with its source; that is the only intentional exception and is not a conflict error. `FINGERPRINT_SEED_CONFLICT` is not part of the contract.
- Import exception: importing an exported profile preserves the exported seed so the fingerprint remains reproducible; two profiles sharing a seed after import is allowed and is not an error.
- Inputs: 创建或复制档案请求、导入载荷、密码学随机源。
- Outputs: 非零种子；复制总是得到新种子；不得使用时间戳、邮箱、档案名称或其他可预测值作为种子；种子不被视为秘密。

### REQ-FP-003

- The system MUST produce identical launch argv for identical kernel version, seed, explicit fields, and capability manifest.
- Inputs: 规格、内核版本、能力清单。
- Outputs: 参数数组逐项一致；构建过程不得读取当前时间、随机数或宿主语言环境来改变结果。

### REQ-FP-004

- The system MUST enforce coherence between platform, User-Agent, User-Agent Client Hints, and platform version.
- Inputs: `platform`、`platform_version`、品牌与内核版本。
- Outputs: 可接受的平台与版本组合；`windows`、`macos`、`linux` 与 UA/UA-CH 不一致的组合必须被拒绝。

### REQ-FP-005

- The system MUST validate brand and brand version against the selected platform and kernel major version.
- Inputs: `brand`、`brand_version`、内核版本、平台。
- Outputs: 支持的品牌和版本；品牌版本不得高于内核主版本，平台不支持的品牌必须被拒绝。

### REQ-FP-006

- The system MUST validate languages and timezone as a coherent locale set and MUST reject empty, duplicate, malformed, or unsupported values.
- Inputs: `languages`、`timezone`、可选代理地区。
- Outputs: 规范化后的语言列表与 IANA 时区；非法值被拒绝；代理地区与语言时区不一致时返回警告而非静默修改。

### REQ-FP-007

- The system MUST restrict `hardware_concurrency` to a realistic allowed set per platform, sourced from the manifest `flagSpec.per_platform_values` for the CPU flag when present and otherwise from its `values` set, and MUST reject zero, negative, or implausibly large values.
- Inputs: CPU 核心数、平台、能力清单。
- Outputs: 允许值列表中的整数；清单声明 `per_platform_values` 时以其为权威，缺失时用 `values`，两者不得合并放宽集合；两个来源都缺失的 CPU 开关是无效清单；越界值被拒绝并给出可选范围。

### REQ-FP-008

- The system MUST default to a host-aligned persona and MUST require explicit confirmation for a cross-OS persona.
- Inputs: 宿主平台、用户选择的 `platform`。
- Outputs: 默认预设与宿主一致；跨系统选择返回 `reduced_assurance=true` 和持久化警告标记；未确认的跨系统规格不得启动。

### REQ-FP-009

- The system MUST validate every spec field and value against the installed kernel capability manifest before saving or launching.
- Capability shape: the manifest declares `flags` (including required `window-size` of type `string` with a non-empty candidate set), `sandbox_kind` (`namespace|seatbelt|win32-restricted-token|none`), `network_controls` with `doh_mode: disabled|unsupported` and `ipv6_mode: block|unsupported`, and `flagSpec.values` or `per_platform_values` for data-driven platform restrictions. `per_platform_values` must cover `windows`, `macos` and `linux`, with a non-empty array for each; empty or partial maps are invalid manifests.
- `unsupported` semantics: a manifest that declares `unsupported` is valid; it means the artifact cannot enforce that control on that platform. Validation MUST treat it as a missing capability and refuse the affected launch rather than marking the manifest invalid or silently relaxing the spec.
- Inputs: 规格、能力清单、内核版本。
- Outputs: 支持的字段与取值；清单外字段、无效果开关和不支持的取值必须被拒绝，不得静默丢弃；清单声明 `per_platform_values` 时以其为权威，否则使用 `values`；`window-size` 缺失或没有候选集合时清单无效。

### REQ-FP-010

- The system MUST NOT expose screen dimensions, device pixel ratio, color depth, font whitelist, or plugin enumeration as fields that the product can spoof.
- Inputs: 规格编辑请求、宿主环境信息。
- Outputs: 这些字段不在可编辑规格中；UI 与 API 显示宿主实际值和 host-bound 限制说明（含 `navigator.plugins`）；尝试设置必须被拒绝或明确标记为只读。

### REQ-FP-011

- The system MUST map spoof toggles only to categories supported by the capability manifest and MUST default all supported categories to enabled. `spoof` is optional; an omitted object and omitted categories mean enabled, and only an explicit `false` maps to the corresponding `--disable-spoofing` value. Capability manifests MAY advertise a category without effect; such categories MUST NOT be exposed as editable and MUST be rejected if supplied.
- Inputs: 可选 `spoof` 对象、能力清单。
- Outputs: 对应的 `--disable-spoofing` 参数或空；未声明的类别、无效果类别被拒绝，不得静默丢弃或自动改成 false；全部关闭时产生显式警告。

### REQ-FP-012

- The system MUST build launch argv as a typed argument array, MUST NOT invoke a shell, and MUST reject duplicate or conflicting arguments.
- Inputs: 规格、内核安装记录（含解包后可执行文件路径与 `binary_sha256`）、用户数据目录、每运行档案回环 HTTP 入站、显示与运行模式。
- Outputs: 确定性参数数组；每个内核开关最多出现一次；多个 `spoof` 类别关闭时必须合并为单个 `--disable-spoofing=<category>[,<category>...]` 参数，类别按 `font`、`audio`、`canvas`、`clientrects`、`gpu` 固定顺序排列；`window` 只在 `window-size` 参数中出现一次；敏感值不进入 argv。

### REQ-FP-013

- The system MUST version the spec schema and MUST preserve unknown fields or reject loading instead of silently discarding them.
- Inputs: 旧版本规格、导入文件、升级后的 schema。
- Outputs: 成功迁移并记录原版本，或返回不兼容错误；迁移不得改变种子和显式字段语义。

### REQ-FP-014

- The system MUST revalidate a profile spec when its bound kernel version changes and MUST report which derived traits may change.
- Inputs: 档案当前内核、目标内核、能力清单差异。
- Outputs: 兼容结论与差异列表；不兼容时阻止迁移；用户确认后保留可回滚记录。

### REQ-FP-015

- The system MUST record the kernel version, spec schema version, and seed needed to reproduce a profile's fingerprint.
- Inputs: 档案创建、复制、迁移和启动。
- Outputs: 档案详情可显示复现三要素；导出包含这些字段；种子不被视为秘密，但不得与凭据混存。

### REQ-FP-016

- The system MUST classify probe-observable traits as explicit, derived, or host-bound and MUST NOT claim control over host-bound traits. `navigator.plugins`/`mimeTypes`, screen metrics, `devicePixelRatio`, color depth and font enumeration are host-bound.
- Inputs: 指纹探针结果、规格、宿主环境。
- Outputs: 测试报告按三类归因；宿主绑定字段（含 `navigator.plugins`）的差异不得被误报为规格失败，也不得计入跨种子差异率。

### REQ-FP-017

- The system MUST require `network.doh_mode=disabled` and `network.ipv6_mode=block` for proxied profiles, and MUST verify that the selected kernel manifest declares the corresponding controls before launch.
- Inputs: 指纹规格、内核能力清单、启动参数、启动探针。
- Outputs: 有效配置中 DoH 不绕过代理且 IPv6 直连被阻止；清单声明 `doh_mode=unsupported` 或 `ipv6_mode=unsupported` 时清单仍合法，但代理档案 MUST NOT 启动，返回 `KERNEL_UNSUPPORTED`（422）且不得静默降级为直连；直连档案不创建代理入站或出站，仍应用内核支持的 DoH/IPv6/非代理 UDP 标志，并在响应中返回 `direct=true` 与 `reduced_assurance=true`，不声称代理级远程 DNS、DoH 或 IPv6 失败关闭；对应内核标志无法应用时 MUST NOT 启动并返回稳定错误，不得仅记录为已声明限制；探针失败同样阻止启动。

### REQ-FP-018

- The system MUST treat `webrtc.disable_non_proxied_udp=false` as an advanced unsafe setting, requiring explicit confirmation and an audit action before persistence.
- Inputs: 指纹规格、确认令牌、操作者、审计上下文。
- Outputs: 默认值为 `true`；关闭时写入 `proxy.non_proxied_udp.disable` 审计事件并在 UI 持续显示风险。

### REQ-FP-019

- The system MUST derive the expected sandbox mechanism from the manifest `sandbox_kind` (`namespace|seatbelt|win32-restricted-token|none`), replacing the former `requires_namespace_sandbox` boolean, and MUST require the effective sandbox to be verified before any proxy resource is created or the kernel is spawned.
- Inputs: 清单 `sandbox_kind`（Linux `namespace` 同时要求强制 seccomp profile）、宿主/容器配置、启动计划、沙箱探测结果、`ALLOW_UNSANDBOXED`。
- Outputs: 沙箱生效时写入 `isolation=enforced`；任何宿主无法确认时返回 `SANDBOX_UNAVAILABLE` 且不创建出站、入站、路由规则或子进程；服务端显式 `ALLOW_UNSANDBOXED=1` 且通过包含性探针与独立 UID/网络边界时才继续启动，写 `isolation=unsandboxed` 并记录审计与启动警告；`sandbox_kind=none` 按无沙箱处理，桌面端直接返回 `SANDBOX_UNAVAILABLE`，服务端只有通过上述 `ALLOW_UNSANDBOXED` 门禁才启动；桌面端无法确认 seatbelt/restricted-token 时返回 `SANDBOX_UNAVAILABLE`，不存在桌面无沙箱例外。
- Semantics: 启动事务顺序、`runtimes.isolation` 持久化与 proof strip 推导由 `browser-runtime` REQ-RUNTIME-014/018 定义。

### REQ-FP-020

- The system MUST resolve the launch executable from the kernel install record and MUST NOT use the manifest `sha256` as the executable identity; the extracted executable MUST be verified against `binary_sha256` before spawn.
- Inputs: 内核版本、安装记录 `asset_sha256`/`binary_sha256`、实际可执行文件。
- Outputs: 文件缺失返回 `KERNEL_NOT_FOUND`；可执行文件摘要与 `binary_sha256` 不符返回 `KERNEL_CHECKSUM_MISMATCH`，即使发布资产摘要匹配也不得启动；失败不创建代理资源或子进程；清单 `sha256` 只用于下载与安装校验。
- Semantics: 校验执行点由 `browser-runtime` REQ-RUNTIME-004 定义，本要求约束启动计划与参数构建不得把发布资产摘要当作可执行身份。

## Verification

### VER-FP-001

- Method: 对合法规格、未知字段、缺失字段、错误类型和越界值执行 schema 校验；分别在档案 payload、启动偏好、导出与数据库中搜索窗口尺寸。
- covers: `REQ-FP-001`
- Pass condition: 合法规格通过；非法输入返回字段级错误；未知字段不会被静默接受；`spoof` 省略时等价于全部启用；`window` 只存在于指纹规格并映射为单个 `window-size` 参数；档案 payload、启动偏好、导出与数据库均无顶层 `window`/`window_json`。

### VER-FP-002

- Method: 批量创建与复制档案，检查随机源调用与可预测输入；导入包含已知种子的导出文件。
- covers: `REQ-FP-002`
- Pass condition: 创建与复制均生成非零新种子，`copy_fingerprint=true` 不复用源种子；相同时间或名称不产生相同种子；不存在 `FINGERPRINT_SEED_CONFLICT` 返回路径；导入保留导出种子且可复现指纹，同种子不再报错。

### VER-FP-003

- Method: 对固定规格重复构建 argv 100 次，并改变系统时间与区域设置。
- covers: `REQ-FP-003`
- Pass condition: 100 次输出逐项相同；环境变化不改变参数数组。

### VER-FP-004

- Method: 构造平台、UA 与 UA-CH 不一致的组合并启动探针。
- covers: `REQ-FP-004`
- Pass condition: 不一致组合被拒绝；通过校验的档案在探针中平台字段一致。

### VER-FP-005

- Method: 测试每个品牌在 Windows、macOS、Linux 上的版本组合，包括高于内核主版本的版本。
- covers: `REQ-FP-005`
- Pass condition: 支持组合通过；不支持组合被拒绝；品牌版本不高于内核主版本。

### VER-FP-006

- Method: 使用空、重复、格式错误和未知语言与时区，并构造代理地区冲突。
- covers: `REQ-FP-006`
- Pass condition: 非法值被拒绝；合法值被规范化；地区冲突返回警告且不修改用户值。

### VER-FP-007

- Method: 测试 0、负数、1、2、4、8、16、64、256 等 CPU 核心数，并使用带 `per_platform_values`、仅带 `values`、两者都缺失三种清单。
- covers: `REQ-FP-007`
- Pass condition: 仅允许集合中的值通过；声明 `per_platform_values` 时以其为权威且不与 `values` 合并，缺失时使用 `values`；两者都缺失的清单被拒绝；越界值返回可选范围。

### VER-FP-008

- Method: 在 macOS 与 Linux 宿主上创建默认规格和跨系统规格。
- covers: `REQ-FP-008`
- Pass condition: 默认平台与宿主一致；跨系统规格带降低保证标记；未确认时启动被拒绝。

### VER-FP-009

- Method: 使用缺少某开关、取值受限、声明无效果、`sandbox_kind=none` 与 `network_controls` 为 `unsupported` 的能力清单校验规格，并移除 `window-size`。
- covers: `REQ-FP-009`
- Pass condition: 清单外字段被拒绝；无效果开关不出现在 UI；错误信息指出具体能力；缺少 `window-size` 或候选集合的清单被判无效；`unsupported` 清单仍合法但影响对应启动；`per_platform_values` 为权威，否则 `values` 为权威。

### VER-FP-010

- Method: 尝试通过 API 写入 `screen_width`、`device_pixel_ratio`、`color_depth`、字体列表和插件列表。
- covers: `REQ-FP-010`
- Pass condition: 写入被拒绝或字段只读；UI 显示宿主值、字体/插件 host-bound 限制说明；不影响已保存规格。

### VER-FP-011

- Method: 逐个关闭 font、audio、canvas、clientrects、gpu，测试省略整个 `spoof`、省略单个类别、不支持类别与无效果类别。
- covers: `REQ-FP-011`
- Pass condition: 只有显式 `false` 生成对应 `--disable-spoofing`；省略时默认全部启用；不支持或无效果类别被拒绝；全部关闭时出现警告。

### VER-FP-012

- Method: 对包含空格、引号、Unicode 和特殊字符的路径与参数执行构建，并检查参数数组；分别从指纹规格与伪造的档案字段提供窗口尺寸。
- covers: `REQ-FP-012`
- Pass condition: 参数按数组传递，不经过 shell；无重复开关；`window-size` 只出现一次且只来自指纹规格；敏感值不出现在 argv。

### VER-FP-013

- Method: 加载旧 schema、未知字段和损坏规格，执行迁移或拒绝。
- covers: `REQ-FP-013`
- Pass condition: 可迁移规格保留种子和显式字段；不可迁移规格返回错误；未知字段不被静默删除。

### VER-FP-014

- Method: 将档案从 148 迁移到候选内核，比较能力清单并执行探针。
- covers: `REQ-FP-014`
- Pass condition: 差异被列出；不兼容时阻止迁移；确认后保留回滚记录和原复现三要素。

### VER-FP-015

- Method: 导出档案、重新导入并读取详情中的内核版本、schema 版本和种子。
- covers: `REQ-FP-015`
- Pass condition: 三要素完整且一致；重新导入后可用相同内核复现；默认导出不含代理凭据。

### VER-FP-016

- Method: 对同一档案执行探针，按 explicit、derived、host-bound 分类比较结果。
- covers: `REQ-FP-016`
- Pass condition: 显式字段稳定且符合规格；派生字段在同一内核与种子下稳定；宿主绑定字段（`screen.*`、`devicePixelRatio`、字体、`navigator.plugins`）差异被正确归因，且插件字段不参与跨种子差异率。

### VER-FP-017

- Method: 使用 `doh_mode=disabled`/`ipv6_mode=block`、值为 `unsupported`、以及探针失败三种内核清单分别启动代理档案与直连档案，运行 DNS/IPv6 探针并检查启动结果与错误。
- covers: `REQ-FP-017`
- Pass condition: 声明并生效时启动成功且无 DoH/IPv6 绕过；`unsupported` 清单是合法 fixture，但代理启动返回 `KERNEL_UNSUPPORTED` 且无直连回退；直连档案返回 `direct=true`/`reduced_assurance=true`，标志无法应用或探针失败时拒绝启动；探针失败同样拒绝启动。

### VER-FP-018

- Method: 尝试直接 PATCH 关闭非代理 UDP 保护，再使用高级确认写入，检查审计和 UI。
- covers: `REQ-FP-018`
- Pass condition: 无确认时被拒绝；确认后写入审计事件并显示风险；默认值始终为 `true`。

### VER-FP-019

- Method: 在 `sandbox_kind=namespace` 且 seccomp 生效、无法启用命名空间沙箱、`ALLOW_UNSANDBOXED=1` 显式无沙箱、以及 `sandbox_kind=none` 四种配置下启动，检查启动顺序、代理资源与 `isolation`。
- covers: `REQ-FP-019`, `REQ-RUNTIME-014`
- Pass condition: 沙箱校验发生在创建代理资源与启动内核之前；生效时 `isolation=enforced`；任何宿主无法确认时 `SANDBOX_UNAVAILABLE` 且无出站/入站/规则/子进程；服务端显式无沙箱且通过包含性探针时 `isolation=unsandboxed` 并写审计与警告；`sandbox_kind=none` 在桌面端被拒绝，在服务端只有通过 `ALLOW_UNSANDBOXED` 门禁才以 `isolation=unsandboxed` 启动；桌面无法确认时拒绝启动。

### VER-FP-020

- Method: 使用安装记录启动，分别删除可执行文件、篡改可执行文件但保持发布资产摘要不变，并检查进程与代理资源。
- covers: `REQ-FP-020`, `REQ-RUNTIME-004`
- Pass condition: 缺失返回 `KERNEL_NOT_FOUND`；可执行文件摘要与 `binary_sha256` 不符返回 `KERNEL_CHECKSUM_MISMATCH`，即使 `asset_sha256`/清单 `sha256` 匹配也不启动；失败无子进程、入站、路由或出站引用残留。

## Related ADRs

- [ADR-0002：固定 fingerprint-chromium 148 并建立内核准入](../../adr/0002-pin-kernel-148.md)
- [ADR-0006：显式指纹规格与种子派生模型](../../adr/0006-fingerprint-spec-model.md)
- [ADR-0009：单实例 Xray 动态协议引擎与连接复用](../../adr/0009-shared-xray-engine.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- [`../../contracts/fingerprint-spec.schema.json`](../../contracts/fingerprint-spec.schema.json)
- <https://github.com/adryfish/fingerprint-chromium>
