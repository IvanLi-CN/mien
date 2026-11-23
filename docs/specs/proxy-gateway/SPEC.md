# 正向代理协议与网络泄漏防护

> 本文件是代理主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: fingerprint-chromium 只向浏览器暴露 HTTP、HTTPS 与 SOCKS 代理入口，且 `--proxy-server` 不支持认证凭据。产品必须支持 VLESS、VMess、Shadowsocks、Trojan、Hysteria2 等主流正向代理协议，同时保证凭据不进入浏览器进程、目标域名不本地泄漏、引擎故障时失败关闭。正向代理节点可能有连接数上限或共享风控，网络层不能为每个档案建立独立连接池。
- In scope: 代理配置 schema、协议矩阵、分享链接导入、单实例共享 Xray 协议引擎、每运行档案 HTTP 入站与路由、共享出站与引用计数、连接复用、动态配置、远程 DNS、WebRTC/IPv6/DoH 防护、代理检查、脱敏、直接模式。
- Out of scope: 订阅 URL 定时刷新与多节点调度（后续范围）、指纹字段（由 `fingerprint-engine` 负责）、内核进程监督（由 `browser-runtime` 负责）、代理服务销售。

## Terms and Interfaces

- `proxy profile`: 一个带类型的正向代理配置，包含 `kind`、`server`、`port`、非敏感 `options` 和加密凭据。
- `protocol engine`: 产品固定并校验的 Xray-core 可执行文件，每个宿主一个共享进程，负责 VLESS、VMess、Shadowsocks、Trojan、Hysteria2 等协议、认证、TLS/REALITY/WS/gRPC/QUIC 与出站 DNS。
- `proxy gateway`: 核心库中的监督组件，负责配置校验、Xray 管理 API 调用、共享出站引用计数、入站/路由生命周期、就绪探测、健康事件和端口生命周期；不转发字节流。
- `runtime inbound`: Xray 在 `127.0.0.1` 上为单个运行档案创建的 HTTP 入站；Chromium 只连接该入口。
- `shared outbound`: 由 `outbound_key` 标识的 Xray 出站；相同节点、凭据、传输、TLS/REALITY 与 mux 配置的档案共享它及其连接池。
- `route rule`: 将 runtime inbound 的 `inboundTag` 指向 shared outbound 的 Xray 路由规则。
- `bootstrap resolution`: 仅对代理节点主机名进行的本地解析；目标域名不得本地解析。
- `Tier A protocol`: 首个发布必须通过发布门禁的协议：VLESS、VMess、Shadowsocks、Trojan。
- `Tier B protocol`: 受支持但按能力清单和独立测试标注的协议：Hysteria2、WireGuard；TUIC 不进入 v1。
- `share link`: `vless://`、`vmess://`、`ss://`、`trojan://`、`hysteria2://` 形式的单节点配置链接。
- `fail-closed`: 引擎不可用、配置无效或出站失败时，浏览器失去网络，而不是回退直连或其他代理。
- Interface: 内核启动参数 `--proxy-server=http://127.0.0.1:<proxy_port>`；`proxies` 表；`/api/v1/proxies` 系列端点；Xray 管理 API 与出站/入站/路由配置。

## Requirements

### REQ-PROXY-001

- The system MUST support the Tier A protocols VLESS, VMess, Shadowsocks (AEAD and 2022 methods, plugins when declared), and Trojan, and MUST support the Tier B protocols Hysteria2 and WireGuard when declared by the installed engine capability manifest. TUIC MUST be rejected in v1.
- Inputs: 代理协议类型、服务器地址、端口、协议选项、凭据、引擎能力清单。
- Outputs: 可生成的引擎出站配置；未在清单中声明或未通过测试的协议必须被拒绝，不得静默降级为 HTTP/SOCKS。

### REQ-PROXY-002

- The system MUST store and validate proxy configurations as typed data, including protocol kind, server, port, protocol options, transport, TLS/REALITY settings, and encrypted credentials.
- Inputs: 用户编辑、分享链接导入、导入文件、API 请求。
- Outputs: 通过 schema 与能力清单校验的配置；未知字段、越界值、非法端口、缺失凭据和不支持的传输组合必须被拒绝并返回字段级错误。

### REQ-PROXY-003

- The system MUST run exactly one shared protocol engine per host, create one loopback HTTP inbound per running profile, add a route rule from that inbound to the selected outbound, and wait for the inbound to become ready before spawning the browser.
- Inputs: 运行时启动/停止、代理配置、端口分配、共享引擎状态。
- Outputs: 单实例引擎；每运行档案独立的 `proxy_port`、入站标签和路由规则；共享出站与引用计数；引擎失败时浏览器不得启动；运行时结束后入站和规则被删除，出站按引用归零删除。

### REQ-PROXY-004

- The system MUST verify the engine binary version and SHA-256 against the installed engine manifest before first use, and MUST refuse to launch on mismatch.
- Inputs: 引擎版本、平台、期望校验和、实际文件。
- Outputs: 校验通过或稳定错误 `PROXY_ENGINE_CHECKSUM_MISMATCH`；不创建入站、出站或路由。

### REQ-PROXY-005

- The system MUST dynamically add a loopback HTTP inbound and a route rule for each runtime, and MUST add or reuse a shared outbound keyed by node, credential, transport, TLS/REALITY and mux settings. The default route MUST be block.
- Inputs: 代理配置、端口、DNS 策略、引擎能力清单、共享引擎状态。
- Outputs: 通过 Xray API 创建的入站、出站和规则；不生成浏览器流量可走的直连出站；同一 `outbound_key` 只存在一个出站。

### REQ-PROXY-006

- The system MUST pass credentials to the shared engine only through the loopback management API or a protected bootstrap channel, MUST NOT place credentials in the engine argv, logs, events, or API responses, and MUST NOT persist a credential-bearing engine config.
- Inputs: 代理凭据、运行时目录、管理 API、日志采集。
- Outputs: 凭据仅在受保护进程内存与本地 API 中短暂出现；任何可读输出只显示凭据是否已设置；引擎重启时从加密数据库重新生成。

### REQ-PROXY-007

- The system MUST expose only the per-runtime loopback HTTP endpoint to Chromium and MUST NOT place proxy credentials or protocol secrets in the browser argv or profile data.
- Inputs: 每运行档案 HTTP 入站端口、内核启动参数。
- Outputs: `--proxy-server=http://127.0.0.1:<proxy_port>`；内核配置中无 UUID、密码、私钥或 TLS/REALITY 秘密。

### REQ-PROXY-008

- The system MUST resolve target destination domains through the proxy, and MUST NOT perform local DNS resolution for target hosts.
- Inputs: 浏览器请求、协议出站、DNS 策略、bootstrap 配置。
- Outputs: 目标域名由代理协议携带或经代理内 DNS 解析；仅代理节点主机名允许本地 bootstrap 解析；目标域名不出现在本机 DNS 查询中。

### REQ-PROXY-009

- The system MUST fail closed when the engine is unavailable, configuration validation fails, authentication fails, or the upstream node cannot be reached.
- Inputs: 引擎状态、配置检查结果、出站错误、连接超时。
- Outputs: 浏览器网络失败；不得回退直连、系统代理、其他节点或 HTTP/SOCKS 降级；UI 显示稳定错误类。

### REQ-PROXY-010

- The system MUST pass `--disable-non-proxied-udp` by default for every proxied profile and MUST require explicit advanced confirmation with an audit event to omit it.
- Inputs: WebRTC 策略、档案配置、审计上下文。
- Outputs: 默认阻止非代理 UDP；高级关闭时记录操作者、时间和档案；UI 持续显示风险标记；Hysteria2 的 QUIC 仅用于引擎到节点，不改变浏览器非代理 UDP 策略。

### REQ-PROXY-011

- The system MUST prevent IPv6 direct egress when the selected protocol or node cannot proxy IPv6, and MUST surface the resulting limitation.
- Inputs: 协议能力、节点地址族、宿主或容器 IPv6 配置、连接尝试。
- Outputs: IPv6 被代理或被阻止；不出现绕过代理的 IPv6 直连；限制在 UI 与测试报告中可见。

### REQ-PROXY-012

- The system MUST disable DNS-over-HTTPS bypass for proxied profiles and MUST verify the effective configuration during startup or acceptance testing.
- Inputs: 内核能力清单、浏览器策略、测试探针。
- Outputs: DoH 不绕过代理；无法禁用或无法验证时必须阻止启动或显示明确限制，不得静默忽略。

### REQ-PROXY-013

- The system MUST provide a proxy check through temporary inbound, route and outbound resources in the shared engine, then remove those resources; it MUST NOT start a second engine for the check.
- Inputs: 代理配置、共享引擎管理 API、可选回显端点、超时。
- Outputs: 成功/失败、延迟、可选出口 IP 与地区、稳定错误类；检查不记录凭据、UUID、完整节点 URL 或目标 URL；临时资源在成功或失败后都被删除。

### REQ-PROXY-014

- The system MUST map engine and upstream failures to stable error classes, including configuration invalid, engine unavailable, authentication failure, TLS/REALITY failure, DNS failure, connection refused, timeout, and protocol unsupported.
- Inputs: 配置检查、引擎退出、出站错误、超时。
- Outputs: 稳定错误码与可读消息；UI 可区分配置、引擎、节点和目标站点问题；不泄露秘密。

### REQ-PROXY-015

- The system MUST support importing a single share link for `vless://`, `vmess://`, `ss://`, `trojan://`, and `hysteria2://` without fetching the network or executing code.
- Inputs: 分享链接文本、当前引擎能力清单。
- Outputs: 通过解析和校验的代理草稿；不支持的协议、传输或参数返回具体错误；导入不自动保存或启动。

### REQ-PROXY-016

- The system MUST return a redacted proxy view to UI and API clients, exposing non-secret options but only `credential_set` booleans and credential field names for secrets.
- Inputs: 已保存代理配置、请求者权限。
- Outputs: 无 UUID、密码、私钥、完整分享链接或节点 URL 中的敏感查询参数；编辑时可替换凭据但不能读回旧值。

### REQ-PROXY-017

- The system MUST bind the Xray management API and every runtime HTTP inbound to `127.0.0.1` only, allocate a stable per-runtime port, and document the residual risk that same-user local processes can use the loopback endpoints.
- Inputs: 端口分配、运行时生命周期、本机进程、管理 API。
- Outputs: 非回环地址无法连接；运行时结束后入站端口关闭；管理 API 不暴露到容器外；安全文档记录同用户残余风险。

### REQ-PROXY-018

- The system MUST treat direct mode as an explicit risky configuration and MUST NOT create a runtime inbound/outbound for a direct profile.
- Inputs: 档案代理模式、启动请求。
- Outputs: 直连档案不创建入站/出站；UI 与 API 标记 `direct=true` 和风险说明；不得把直连伪装成代理。

### REQ-PROXY-019

- The system MUST capture engine logs with redaction and rotation, and MUST NOT log full configuration, credentials, share links, or target URLs.
- Inputs: 引擎 stdout/stderr、错误、连接事件。
- Outputs: 结构化或行式日志；单文件上限 10 MB，最多 5 个文件；敏感值不出现；可按档案诊断协议与连接错误。

### REQ-PROXY-020

- The system MUST publish a protocol engine capability manifest containing version, platform, checksum, license, supported protocols, transports, TLS modes, connection-reuse support, and known limitations, and MUST use it to drive UI and validation.
- Inputs: 引擎安装包、构建清单、测试结果。
- Outputs: 机器可读清单；UI 不展示未声明能力；协议组合按清单拒绝或标注；升级前可比对差异。

### REQ-PROXY-021

- The system MUST share an outbound and its connection pool among running profiles with the same stable `outbound_key`; profiles with different credentials, transport, TLS/REALITY or mux settings MUST NOT share.
- Inputs: 代理配置、凭据引用、传输设置、mux 设置、运行档案。
- Outputs: 每个 key 一个出站；引用计数准确；最后一个引用释放时删除出站；复用能力由引擎清单声明。

### REQ-PROXY-022

- The system MUST fail closed when the shared engine crashes, the management API is unavailable, or rebuilding the running configuration fails; it MUST restart with bounded backoff and recreate inbounds on their original `proxy_port`.
- Inputs: 引擎 PID、API 健康、运行档案状态、原端口。
- Outputs: 所有运行档案网络失败；全局 `proxy.engine.failed` 事件；重建成功产生 `proxy.engine.rebuilt`；重建失败把受影响运行时标记为 `failed`。

### REQ-PROXY-023

- The system MUST bind the Xray management API to loopback only, serialize mutations, make add/remove idempotent, and reconcile outbound refcounts after restart.
- Inputs: 管理 API 地址、并发启动/停止请求、引擎重启。
- Outputs: 无重复出站、无孤儿入站或规则、引用计数可重建；API 不暴露到非回环地址。

## Verification

### VER-PROXY-001

- Method: 对 Tier A 与 Tier B 的每种协议启动受控节点，通过共享引擎创建入站/出站并通过浏览器访问探针页。
- covers: `REQ-PROXY-001`
- Pass condition: Tier A 全部通过；Hysteria2/WireGuard 按清单通过或明确标注；TUIC 被拒绝；无静默降级。

### VER-PROXY-002

- Method: 使用合法配置、未知字段、错误端口、缺失凭据、非法传输组合和超长字段执行 schema 校验。
- covers: `REQ-PROXY-002`
- Pass condition: 合法配置通过；非法输入返回字段级错误；不产生入站、出站或规则。

### VER-PROXY-003

- Method: 启动两个使用不同协议的档案，再启动第三个使用相同节点/凭据的档案，检查进程、端口、出站复用和停止顺序。
- covers: `REQ-PROXY-003`
- Pass condition: 全宿主只有一个 Xray 引擎；每个运行档案有独立入站/端口/规则；相同 key 的档案共享出站；停止后入站和规则释放、引用归零时出站删除。

### VER-PROXY-004

- Method: 篡改引擎二进制、删除清单、替换校验和。
- covers: `REQ-PROXY-004`
- Pass condition: 所有不匹配拒绝启动，返回 `PROXY_ENGINE_CHECKSUM_MISMATCH`，无入站、出站或规则残留。

### VER-PROXY-005

- Method: 检查由 Xray API 创建的入站、出站和规则，并尝试构造直连或未匹配流量。
- covers: `REQ-PROXY-005`
- Pass condition: 每个运行档案有一个 HTTP 入站和一条路由规则；默认出站为 block；无直连回退；同一 key 只有一个出站。

### VER-PROXY-006

- Method: 启动后扫描 argv、日志、事件、API 响应和持久化文件，检查凭据是否只经回环 API 传递。
- covers: `REQ-PROXY-006`
- Pass condition: 凭据不出现在 argv、日志、事件、API 响应或持久化引擎配置；重启可重新生成；管理 API 只绑定回环。

### VER-PROXY-007

- Method: 检查 Chromium argv、profile 配置、运行档案入站和 Xray 出站中的秘密分布。
- covers: `REQ-PROXY-007`
- Pass condition: 浏览器只看到本运行档案的回环 HTTP 地址；无 UUID、密码、私钥或 REALITY 秘密。

### VER-PROXY-008

- Method: 使用 DNS canary 目标，监控本机 DNS 与节点侧解析。
- covers: `REQ-PROXY-008`
- Pass condition: 本机无目标域名查询；节点侧收到域名或完成解析；仅节点主机名可能本地 bootstrap 解析。

### VER-PROXY-009

- Method: 关闭共享引擎、使用错误凭据、断开节点网络、使配置非法。
- covers: `REQ-PROXY-009`
- Pass condition: 所有场景浏览器网络失败；无直连或其他节点回退；UI 显示对应错误类。

### VER-PROXY-010

- Method: 默认启动并读取 argv，再通过高级开关关闭保护并检查审计。
- covers: `REQ-PROXY-010`
- Pass condition: 默认包含 `--disable-non-proxied-udp`；高级关闭需确认且有审计；风险标记持续显示。

### VER-PROXY-011

- Method: 在启用 IPv6 的容器中通过支持和不支持 IPv6 的节点访问 IPv6-only 与双栈目标，抓取直连尝试。
- covers: `REQ-PROXY-011`
- Pass condition: IPv6 被代理或被阻止；无直连 IPv6；限制在 UI 中可见。

### VER-PROXY-012

- Method: 在探针中检查 DoH 状态和 DNS 路径，尝试触发安全 DNS。
- covers: `REQ-PROXY-012`
- Pass condition: DoH 未绕过代理；无法验证时启动被阻止或明确标记限制。

### VER-PROXY-013

- Method: 对可用节点、错误凭据、错误 TLS/REALITY、超时和不可达节点执行检查，配置与不配置回显端点；检查后扫描临时资源。
- covers: `REQ-PROXY-013`
- Pass condition: 结果与错误类正确；延迟和出口 IP 仅在可用时返回；日志不含秘密；临时入站/出站/规则被删除。

### VER-PROXY-014

- Method: 模拟配置错误、引擎退出、认证失败、TLS/REALITY 失败、DNS 失败、拒绝连接和超时。
- covers: `REQ-PROXY-014`
- Pass condition: 每种失败返回不同稳定错误码；消息可区分配置、引擎、节点和目标问题；无秘密泄露。

### VER-PROXY-015

- Method: 导入合法与非法 VLESS、VMess、Shadowsocks、Trojan、Hysteria2 分享链接，包括不支持参数和超大输入。
- covers: `REQ-PROXY-015`
- Pass condition: 合法链接生成草稿；非法链接返回具体字段错误；不发起网络请求、不自动保存或启动。

### VER-PROXY-016

- Method: 读取代理详情、编辑并替换凭据，扫描 API 与 UI 响应。
- covers: `REQ-PROXY-016`
- Pass condition: 只显示非秘密字段与 `credential_set`；无法读回旧凭据；替换后可保存；无完整分享链接。

### VER-PROXY-017

- Method: 从非回环地址连接管理 API 和运行档案入站，并在运行时结束后重试；检查安全文档。
- covers: `REQ-PROXY-017`
- Pass condition: 非回环连接被拒绝；运行时结束后入站端口关闭；管理 API 不可从非回环访问；文档记录同用户残余风险。

### VER-PROXY-018

- Method: 创建直连档案并启动，检查进程、UI 与 API。
- covers: `REQ-PROXY-018`
- Pass condition: 不创建运行档案入站/出站；`direct=true` 与风险说明可见；无代理成功伪装。

### VER-PROXY-019

- Method: 触发协议错误、认证失败和长时间运行，检查日志内容、权限与轮转。
- covers: `REQ-PROXY-019`
- Pass condition: 日志轮转符合上限；无配置、凭据、分享链接或目标 URL；可定位错误类。

### VER-PROXY-020

- Method: 加载引擎清单，渲染代理编辑器，并尝试提交未声明协议或传输。
- covers: `REQ-PROXY-020`
- Pass condition: 清单字段完整；UI 只显示声明能力；未声明组合被拒绝；升级差异可比较。

### VER-PROXY-021

- Method: 使用相同节点/凭据启动多个档案，再使用不同凭据启动一个档案；检查 Xray 出站、连接和引用计数。
- covers: `REQ-PROXY-021`
- Pass condition: 相同 key 只创建一个出站并复用；不同凭据创建不同出站；最后一个引用释放时删除；日志不泄露凭据。

### VER-PROXY-022

- Method: 运行多个档案时强杀 Xray，观察失败关闭、事件、重启和原端口重建；再模拟重建失败。
- covers: `REQ-PROXY-022`
- Pass condition: 所有运行档案网络失败；产生全局事件；重启后按原端口重建；重建失败标记受影响运行时 `failed`；无直连回退。

### VER-PROXY-023

- Method: 并发启动/停止档案，重启引擎，检查 Xray API 访问控制、幂等和引用计数对账。
- covers: `REQ-PROXY-023`
- Pass condition: 无重复出站、无孤儿入站/规则；引用计数正确；API 仅回环可达；并发操作不产生竞争状态。

## Related ADRs

- [ADR-0003：SQLite 存储与字段级密钥保护](../../adr/0003-storage-and-secret-protection.md)
- [ADR-0007：每档案独立协议引擎（已被 ADR-0009 取代）](../../adr/0007-protocol-engine-forward-proxy.md)
- [ADR-0009：单实例 Xray 动态协议引擎与连接复用](../../adr/0009-shared-xray-engine.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- [`../../contracts/proxy-engine-manifest.schema.json`](../../contracts/proxy-engine-manifest.schema.json)
- <https://github.com/XTLS/Xray-core>
