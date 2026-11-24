# 能力清单与发布资产

本目录记录首个发布候选的内核与 Xray 能力清单。JSON 文件使用 [`../kernel-manifest.schema.json`](../kernel-manifest.schema.json) 和 [`../proxy-engine-manifest.schema.json`](../proxy-engine-manifest.schema.json) 的结构。

清单是**候选清单**：它记录固定版本、固定平台的构件事实与已知缺陷，而不是运行时准入通过证明。Xray、内核、Docker 与协议矩阵 spike 完成后，必须用实测结果更新 `limitations` 与 `known_issues`。

## 1. 校验和语义：发行包与可执行文件

清单中的 `sha256` 只有一个含义：**公开发行包（release asset）的 SHA-256**，来自对应 GitHub Release asset 的 `digest` 字段。它**不是**解压后可执行文件的摘要，也不得用来校验可执行文件。

安装记录必须同时保存两个摘要：

| 字段 | 含义 | 计算时机 | 来源 |
| --- | --- | --- | --- |
| `asset_sha256` | 下载到的发行包（DMG/TAR.XZ/ZIP）摘要 | 下载完成后、解压之前 | 清单 `sha256` 与发布页 `digest` 必须一致 |
| `binary_sha256` | 解压出的内核/引擎可执行文件摘要 | 解压完成后、首次使用之前 | 本地计算，写入安装记录 |

安装与启动流程必须：

1. 下载发行包并校验 `asset_sha256`；
2. 解压到版本目录；
3. 对最终可执行文件/内核二进制计算 `binary_sha256` 并写入实例安装记录；
4. 启动运行时或协议引擎时，用安装记录的 `binary_sha256` 校验实际要执行的二进制；
5. 不把清单中的发行包校验和冒充为最终二进制校验和，也不在启动路径上重新下载或静默修复；
6. 二进制被替换、删除或摘要不符时，走稳定错误码，不启动进程。

| 资产 | 类型 | `asset_sha256` |
| --- | --- | --- |
| fingerprint-chromium 148 macOS ARM64 | DMG | `b72f091e2e1a7583eed389c4b8e3534ed355e568af8c8bbf8fc30a25e23ca679` |
| fingerprint-chromium 148 Linux x86_64 | TAR.XZ | `70d239830332e5820aa34dfcb284161cac0429eee25da642830afe04bda717f4` |
| fingerprint-chromium 148 Windows x64 | ZIP | `9ef3f471b7a6641b4224532522b29141ce3746e27d55788d88e2fd951f362579` |
| Xray-core v26.11.9 macOS ARM64 | ZIP | `233f0c66681cd0726f81a6b2b6ed6a5710869e312047b522e46ed53035bebd26` |
| Xray-core v26.11.9 Linux x86_64 | ZIP | `b651f7e1d9966ee2c1b1ee1bf8b52c72771824d3065dce96bbc8bcc19ed95f82` |

失败映射：

| 情况 | 内核 | 协议引擎 |
| --- | --- | --- |
| 已安装文件缺失 | `KERNEL_NOT_FOUND` | `PROXY_ENGINE_NOT_FOUND` |
| 发行包摘要不符 | `KERNEL_CHECKSUM_MISMATCH` | `PROXY_ENGINE_CHECKSUM_MISMATCH` |
| 可执行文件摘要不符 | `KERNEL_CHECKSUM_MISMATCH` | `PROXY_ENGINE_CHECKSUM_MISMATCH` |

安装记录 `kernel_installs`/`proxy_engine_installs` 同时保存 `asset_sha256` 与 `binary_sha256`；OpenAPI 的 `Kernel`/`Engine` 视图也暴露两者，但 `binary_sha256` 在可执行文件缺失的迁移记录中为 `null`，此时启动返回 `KERNEL_NOT_FOUND`/`PROXY_ENGINE_NOT_FOUND`，视图不得用 `asset_sha256` 冒充。运行时会话与协议引擎能力清单只消费清单，不反向写入清单；清单本身仍然只携带发行包摘要。

## 2. 能力字段

### 2.1 `flags` 与 `per_platform_values`

`flags` 的每个键是内核接受的启动开关，值遵循 `flagSpec`：

- `type`：`uint32`、`integer`、`string`、`boolean` 之一。
- `values`：该开关允许的取值集合；缺省表示形式合法即可。
- `per_platform_values`：按**画像平台**（`windows`、`macos`、`linux`）声明的允许取值集合，用于 CPU 核心数这类按平台受限、且与宿主平台不同的字段。三个平台键都必须存在，且每个数组非空；缺失平台或空数组是无效清单。
- `per_platform_values` 存在时是权威取值来源；不存在时回退到 `values`，但两者不得合并成更宽松的集合。CPU 核心数开关必须至少提供其中一个来源；schema 要求 `fingerprint-hardware-concurrency` 同时满足“有 `per_platform_values`”或“有 `values`”之一。

`window-size`（`type: string`，`WIDTH,HEIGHT`）在所有平台清单中声明，schema 把 `flags.window-size` 设为必填、类型固定为 `string`，并要求非空 `values` 候选集合；缺失或空集合都使清单无效。该候选集合就是 UI 与服务端校验唯一接受的窗口尺寸来源。消费方只能接受集合内值，不能从 UI 预设另造集合。窗口尺寸属于指纹规格的规范字段，**不是屏幕分辨率**：屏幕尺寸、`devicePixelRatio`、颜色深度和字体枚举仍是宿主绑定特征，不由清单或规格伪造（REQ-FP-010）。

语言与时区同样走清单校验：清单声明限制集合时按其校验，未声明时按 BCP-47 与 IANA 规则校验。任何清单外的字段、取值或不生效开关都必须被拒绝，不得静默丢弃（REQ-FP-009）。

### 2.2 `sandbox_kind`

`sandbox_kind` 取代旧的 `requires_namespace_sandbox` 布尔值，取值 `namespace`、`seatbelt`、`win32-restricted-token`、`none`，描述该构件依赖的沙箱机制：

| 平台 | `sandbox_kind` |
| --- | --- |
| `linux-x86_64` | `namespace`（配合强制 seccomp profile） |
| `macos-arm64` | `seatbelt` |
| `windows-x64` | `win32-restricted-token`（restricted token + job object + integrity levels；普通渲染进程不使用 AppContainer） |

- 清单声明的是构件设计依赖；产品仍需在服务流量前核验实际生效的沙箱状态。Docker 中无法确认 `namespace` 沙箱时返回 `SANDBOX_UNAVAILABLE`，服务端只有 `ALLOW_UNSANDBOXED=1` 且通过包含性探针与独立 UID/网络边界时才允许启动并写入审计；桌面（`seatbelt`/`win32-restricted-token`）无法确认时同样拒绝启动，不存在桌面无沙箱例外。Docker 中的 `namespace` 沙箱要求强制 seccomp profile，不能记为“按需”。
- 内核清单的 `headless_chrome_ua_patch` 来自 fingerprint-chromium 148 源码中的 `patches/extra/fingerprint/010-headless.patch` 与 `002-user-agent-fingerprint.patch`：它只把 `HeadlessChrome` UA 改为 `Chrome`，不改变其他 Headless 特征；该字段只存在于内核清单，不属于引擎协议对象。

### 2.3 `network_controls`

`network_controls` 表达内核侧可执行的控制，而非产品意图：

- `doh_mode`：`disabled` 或 `unsupported`；
- `ipv6_mode`：`block` 或 `unsupported`。

`unsupported` 表示该构件在该平台上无法强制执行对应控制。此时要求 `network.doh_mode=disabled` 与 `network.ipv6_mode=block` 的代理档案（REQ-FP-017）不得启动，返回 `KERNEL_UNSUPPORTED`（422），也不得静默降级为直连；这是有效的“能力缺失”清单形态，可用于测试能力缺失路径。当前 `windows-x64` 清单把两项都声明为 `unsupported`，因为 Windows 宿主是后续目标。

### 2.4 已知问题

148 内核清单统一携带 canvas/WebGL 读回一致性与安全问题：

- `#98`：canvas 子区域读回与完整读回可能不一致；`#99`：WebGL `readPixels` 在 1×1 与 8×8 读回间可能不一致。探针报告必须分别归因于内核缺陷，不得判定为指纹规格失败（REQ-FP-016）。
- `#102`：WebGL `readPixels` 打包 16 位格式存在越界访问报告；这是安全缺陷，不是 #98/#99 的读回一致性问题。
- `#108`（仅 `windows-x64`）：CreepJS 在部分 Windows 环境可能卡住；该问题不登记在 macOS/Linux 清单；探针超时不得归因于产品缺陷。

`docs/PRD.md` 记录上游完整问题清单；每个清单只列对该构件探针与运行时行为有影响的问题。发布门禁与测试报告必须引用清单中的编号，而不是口头结论。

### 2.5 协议分级（引擎清单）

`protocols` 是协议名到能力对象的映射，每项包含 `tier`、`transports`、`tls_modes`，可选 `methods`、`plugins`、`obfuscation`、`notes`。`tier` 取值 `A`、`B`、`basic`，且必须与协议名绑定：`vless`/`vmess`/`shadowsocks`/`trojan` 只能为 `A`，`hysteria2`/`wireguard` 只能为 `B`，`http`/`https`/`socks5` 只能为 `basic`。

| 级别 | 协议 | 含义 |
| --- | --- | --- |
| `A` | VLESS、VMess、Shadowsocks、Trojan | 参与发布门禁，必须端到端通过 |
| `B` | Hysteria2、WireGuard | 按清单与独立测试支持，必须明确标注能力与失败原因 |
| `basic` | HTTP、HTTPS、SOCKS5 | 基础正向代理能力，支持用户名密码与远程 DNS，不参与 Tier A/B 门禁 |

- WireGuard 为 Tier B：`transports` 仅 `udp`、`tls_modes` 仅 `none`，需要 UDP 出站与独立端到端验证；失败时必须报错，不得降级到其他协议或直连。
- Hysteria2 为 Tier B：需要 UDP 出站；Docker 部署还需要容器 UDP 许可，macOS 部署只需要宿主 UDP 出口。`obfuscation` 必须显式声明；省略等价于空数组，但发布清单不得省略。Xray v26.11.9 声明为空，含混淆参数的配置必须拒绝，不得默认存在。
- TUIC 不进入 v1，不得出现在任何清单中。
- `connection_reuse: true` 表示 Xray 支持 mux/原生 QUIC 连接复用；具体协议组合与共享出站行为仍需 M0 runtime spike 确认。

## 3. 清单文件

- [`fingerprint-chromium-148.0.7778.215-macos-arm64.json`](fingerprint-chromium-148.0.7778.215-macos-arm64.json)
- [`fingerprint-chromium-148.0.7778.215-linux-x86_64.json`](fingerprint-chromium-148.0.7778.215-linux-x86_64.json)
- [`fingerprint-chromium-148.0.7778.215-windows-x64.json`](fingerprint-chromium-148.0.7778.215-windows-x64.json)
- [`xray-core-v26.11.9-macos-arm64.json`](xray-core-v26.11.9-macos-arm64.json)
- [`xray-core-v26.11.9-linux-x86_64.json`](xray-core-v26.11.9-linux-x86_64.json)

## 4. 更新规则

- 清单按“构件版本 + 目标平台”粒度存在；不同平台的能力差异必须写进各自清单，不得用一份清单代表所有平台。
- 清单变更必须同时更新发行包 `asset_sha256`、`limitations`、`known_issues` 和相关测试。
- `known_issues` 在已发布版本上只增不改语义；修复后在新版本清单中标记，并保留旧版本用于回滚。
- 不得删除已发布清单中的已知缺陷或能力缺失声明。
- 不得把 TUIC 加入 v1 Xray 清单。
- 不得在清单或日志中写入代理凭据、分享链接或密钥。
