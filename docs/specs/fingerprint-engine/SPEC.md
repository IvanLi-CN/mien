# 指纹规格与启动参数

> 本文件是指纹主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: 产品需要把用户可编辑的浏览器画像转换为 fingerprint-chromium 能够理解、且内部一致的启动参数，并保证同一规格可复现。
- In scope: 指纹规格 schema、种子、显式字段、派生特征、一致性规则、能力清单校验、启动参数构建、规格迁移、限制呈现。
- Out of scope: 档案 CRUD 与数据目录（由 `profile-management` 负责）、内核进程监督（由 `browser-runtime` 负责）、代理连接与 DNS（由 `proxy-gateway` 负责）。

## Terms and Interfaces

- `fingerprint spec`: 带 `schema_version` 的结构化规格，包含显式字段和种子。
- `explicit field`: 用户可编辑并通过内核开关表达的字段，例如平台、品牌、语言、时区、CPU 核心数。
- `derived trait`: 由种子和内核版本决定、不逐项编辑的特征，例如 canvas 噪声、音频噪声、WebGL GPU 参数、设备内存。
- `host-bound field`: 受宿主显示、字体或图形栈影响、产品不能可靠伪造的特征，例如 `screen.width`、`devicePixelRatio`、字体枚举。
- `capability manifest`: 内核版本与平台组合实际支持的能力清单。
- Interface: `POST /api/v1/profiles` 与 `PATCH /api/v1/profiles/{id}` 中使用的指纹 JSON；核心函数 `validate_spec`、`build_launch_argv`；内核能力清单 JSON。

## Requirements

### REQ-FP-001

- The system MUST define a versioned fingerprint spec schema containing `schema_version`, `seed`, `platform`, `platform_version`, `brand`, `brand_version`, `hardware_concurrency`, `languages`, `timezone`, `window`, `spoof`, and `webrtc` fields.
- Inputs: 用户编辑、预设、导入文件或复制操作。
- Outputs: 通过 schema 校验的规格；未知字段、缺失必填字段、错误类型和越界值必须被拒绝。

### REQ-FP-002

- The system MUST generate a non-zero 32-bit seed using a cryptographically secure random source and MUST reject a seed already used by any existing profile, including archived profiles.
- Inputs: 创建或复制档案请求、现有种子集合。
- Outputs: 唯一种子或稳定冲突错误；不得使用时间戳、邮箱、档案名称或其他可预测值作为种子。

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

- The system MUST restrict `hardware_concurrency` to a realistic allowed set per platform and MUST reject zero, negative, or implausibly large values.
- Inputs: CPU 核心数、平台。
- Outputs: 允许值列表中的整数；越界值被拒绝并给出可选范围。

### REQ-FP-008

- The system MUST default to a host-aligned persona and MUST require explicit confirmation for a cross-OS persona.
- Inputs: 宿主平台、用户选择的 `platform`。
- Outputs: 默认预设与宿主一致；跨系统选择返回 `reduced_assurance=true` 和持久化警告标记；未确认的跨系统规格不得启动。

### REQ-FP-009

- The system MUST validate every spec field and value against the installed kernel capability manifest before saving or launching.
- Inputs: 规格、能力清单、内核版本。
- Outputs: 支持的字段与取值；清单外字段、无效果开关和不支持的取值必须被拒绝，不得静默丢弃。

### REQ-FP-010

- The system MUST NOT expose screen dimensions, device pixel ratio, color depth, or font whitelist as fields that the product can spoof.
- Inputs: 规格编辑请求、宿主环境信息。
- Outputs: 这些字段不在可编辑规格中；UI 与 API 显示宿主实际值和限制说明；尝试设置必须被拒绝或明确标记为只读。

### REQ-FP-011

- The system MUST map spoof toggles only to categories supported by the capability manifest and MUST default all supported categories to enabled.
- Inputs: `spoof` 对象、能力清单。
- Outputs: 对应的 `--disable-spoofing` 参数或空；不支持的类别被拒绝；全部关闭时产生显式警告。

### REQ-FP-012

- The system MUST build launch argv as a typed argument array, MUST NOT invoke a shell, and MUST reject duplicate or conflicting arguments.
- Inputs: 规格、内核路径、用户数据目录、协议引擎 mixed 入口、显示与运行模式。
- Outputs: 确定性参数数组；每个内核开关最多出现一次；敏感值不进入 argv。

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

- The system MUST classify probe-observable traits as explicit, derived, or host-bound and MUST NOT claim control over host-bound traits.
- Inputs: 指纹探针结果、规格、宿主环境。
- Outputs: 测试报告按三类归因；宿主绑定字段的差异不得被误报为规格失败。

## Verification

### VER-FP-001

- Method: 对合法规格、未知字段、缺失字段、错误类型和越界值执行 schema 校验。
- covers: `REQ-FP-001`
- Pass condition: 合法规格通过；非法输入返回字段级错误；未知字段不会被静默接受。

### VER-FP-002

- Method: 批量生成种子并与现有集合比较，检查随机源调用和可预测输入。
- covers: `REQ-FP-002`
- Pass condition: 所有种子非零且不重复；相同时间或名称不产生相同种子；冲突返回稳定错误。

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

- Method: 测试 0、负数、1、2、4、8、16、64、256 等 CPU 核心数。
- covers: `REQ-FP-007`
- Pass condition: 仅允许集合中的值通过；越界值返回可选范围。

### VER-FP-008

- Method: 在 macOS 与 Linux 宿主上创建默认规格和跨系统规格。
- covers: `REQ-FP-008`
- Pass condition: 默认平台与宿主一致；跨系统规格带降低保证标记；未确认时启动被拒绝。

### VER-FP-009

- Method: 使用缺少某开关、取值受限和声明无效果的能力清单校验规格。
- covers: `REQ-FP-009`
- Pass condition: 清单外字段被拒绝；无效果开关不出现在 UI；错误信息指出具体能力。

### VER-FP-010

- Method: 尝试通过 API 写入 `screen_width`、`device_pixel_ratio`、`color_depth` 和字体列表。
- covers: `REQ-FP-010`
- Pass condition: 写入被拒绝或字段只读；UI 显示宿主值与限制说明；不影响已保存规格。

### VER-FP-011

- Method: 逐个关闭 font、audio、canvas、clientrects、gpu，并测试不支持类别。
- covers: `REQ-FP-011`
- Pass condition: 生成的 `--disable-spoofing` 取值正确；不支持类别被拒绝；全部关闭时出现警告。

### VER-FP-012

- Method: 对包含空格、引号、Unicode 和特殊字符的路径与参数执行构建，并检查参数数组。
- covers: `REQ-FP-012`
- Pass condition: 参数按数组传递，不经过 shell；无重复开关；敏感值不出现在 argv。

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
- Pass condition: 显式字段稳定且符合规格；派生字段在同一内核与种子下稳定；宿主绑定字段差异被正确归因。

## Related ADRs

- [ADR-0002：固定 fingerprint-chromium 148 并建立内核准入](../../adr/0002-pin-kernel-148.md)
- [ADR-0006：显式指纹规格与种子派生模型](../../adr/0006-fingerprint-spec-model.md)
- [ADR-0007：协议引擎承载正向代理](../../adr/0007-protocol-engine-forward-proxy.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- [`../../contracts/fingerprint-spec.schema.json`](../../contracts/fingerprint-spec.schema.json)
- <https://github.com/adryfish/fingerprint-chromium>
