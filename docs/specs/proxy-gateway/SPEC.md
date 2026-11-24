# 正向代理协议与网络泄漏防护

> 本文件是代理主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: fingerprint-chromium 只向浏览器暴露 HTTP、HTTPS 与 SOCKS 代理入口，且 `--proxy-server` 不支持认证凭据。产品必须支持 VLESS、VMess、Shadowsocks、Trojan、Hysteria2、WireGuard 等主流正向代理协议，以及 HTTP/HTTPS/SOCKS5 基础入口，同时保证凭据不进入浏览器进程、目标域名不本地泄漏、引擎故障时失败关闭。正向代理节点可能有连接数上限或共享风控，网络层不能为每个档案建立独立连接池。
- In scope: 代理配置 schema、协议矩阵、分享链接导入、单实例共享 Xray 协议引擎、每运行档案 HTTP 入站与路由、共享出站与引用计数、连接复用、动态配置、远程 DNS、WebRTC/IPv6/DoH 防护、代理检查、脱敏、直接模式。
- Out of scope: 订阅 URL 定时刷新与多节点调度（后续范围）、指纹字段（由 `fingerprint-engine` 负责）、内核进程监督（由 `browser-runtime` 负责）、代理服务销售。

## Terms and Interfaces

- `proxy profile`: 一个带类型的正向代理配置，包含 `kind`、`server`、`port`、非敏感 `options` 和加密凭据。
- `protocol engine`: 产品固定并校验的 Xray-core 可执行文件，每个宿主一个共享进程，负责 VLESS、VMess、Shadowsocks、Trojan、Hysteria2、WireGuard 等协议、认证、TLS/REALITY/WS/gRPC/QUIC 与出站 DNS，以及 HTTP/HTTPS/SOCKS5 基础出站。
- `proxy gateway`: 核心库中的监督组件，负责配置校验、Xray 管理 API 调用、共享出站引用计数、入站/路由生命周期、就绪探测、健康事件和端口生命周期；不转发字节流。
- `runtime inbound`: Xray 在 `127.0.0.1` 上为单个运行档案创建的 HTTP 入站；Chromium 只连接该入口。
- `shared outbound`: 由 `outbound_key` 标识的 Xray 出站；相同节点、凭据、传输、TLS/REALITY 与 mux 配置的档案共享它及其连接池。
- `route rule`: 将 runtime inbound 的 `inboundTag` 指向 shared outbound 的 Xray 路由规则；每条规则携带稳定 `ruleTag`（`rt:<runtime_id>:rule`），删除只按该 tag。
- `bootstrap resolution`: 仅对代理节点主机名进行的本地解析；目标域名不得本地解析。
- `Tier A protocol`: 首个发布必须通过发布门禁的协议：VLESS、VMess、Shadowsocks、Trojan。
- `Tier B protocol`: 受支持但按能力清单和独立测试标注的协议：Hysteria2、WireGuard；TUIC 不进入 v1。
- `basic protocol`: HTTP、HTTPS、SOCKS5 基础正向代理能力，必须可用但不参与 Tier A/B 发布门禁。
- `share link`: `vless://`、`vmess://`、`ss://`、`trojan://`、`hysteria2://` 形式的单节点配置链接。
- `fail-closed`: 引擎不可用、配置无效或出站失败时，浏览器失去网络，而不是回退直连或其他代理。
- Interface: 内核启动参数 `--proxy-server=http://127.0.0.1:<proxy_port>`；`proxies` 表；`/api/v1/proxies` 系列端点；Xray 管理 API 与出站/入站/路由配置。

## Requirements

### REQ-PROXY-001

- The system MUST support the Tier A protocols VLESS, VMess, Shadowsocks (AEAD and 2022 methods, plugins when declared), and Trojan, MUST support the Tier B protocols Hysteria2 and WireGuard when declared by the installed engine capability manifest, and MUST support HTTP, HTTPS and SOCKS5 as basic protocols. TUIC MUST be rejected in v1.
- Inputs: 代理协议类型、服务器地址、端口、协议选项、凭据、引擎能力清单。
- Outputs: 可以生成的引擎出站配置；basic 协议不参与 Tier A/B 门禁；未在清单中声明或未通过测试的协议必须被拒绝，不得静默降级为 HTTP/SOCKS；WireGuard 必须包含隧道内 DNS，且只接受 IPv4/IPv6 字面量，缺失或非法值返回 `PROXY_CONFIG_INVALID`。

### REQ-PROXY-002

- The system MUST store and validate proxy configurations as typed data, including protocol kind, server, port, protocol options, transport, TLS/REALITY settings, and encrypted credentials.
- Inputs: 用户编辑、分享链接导入、导入文件、API 请求。
- Outputs: 通过 schema 与能力清单校验的配置；未知字段、越界值、非法端口、缺失凭据和不支持的传输组合必须被拒绝并返回字段级错误。

### REQ-PROXY-003

- The system MUST run exactly one shared protocol engine per host, create one loopback HTTP inbound per running profile, add a tagged route rule from that inbound to the selected outbound, and wait for the inbound to become ready before spawning the browser.
- Inputs: 运行时启动/停止、代理配置、端口分配、共享引擎状态、沙箱校验结果。
- Outputs: 沙箱校验通过后才创建代理资源，`SANDBOX_UNAVAILABLE` 时不创建入站、出站或规则；单实例引擎；每运行档案独立的 `proxy_port`、入站标签和带稳定 `ruleTag` 的路由规则；共享出站与引用计数；引擎失败时浏览器不得启动；运行时结束后入站和规则被删除，出站按引用归零删除。

### REQ-PROXY-004

- The system MUST verify the engine executable against the installed engine install record before first use: the manifest `sha256` is the release-asset digest, the install record stores `asset_sha256` and `binary_sha256`, and runtime verification uses `binary_sha256` only.
- Inputs: 引擎版本、平台、引擎安装记录、实际可执行文件。
- Outputs: 可执行文件缺失返回 `PROXY_ENGINE_NOT_FOUND`，摘要不符返回 `PROXY_ENGINE_CHECKSUM_MISMATCH`；两种失败都拒绝启动且不创建入站、出站或路由。

### REQ-PROXY-005

- The system MUST dynamically add a loopback HTTP inbound and a route rule for each runtime, and MUST add or reuse a shared outbound keyed by node, credential, transport, TLS/REALITY and mux settings. The default route MUST be block and rule mutations MUST follow REQ-PROXY-024/025.
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
- Outputs: 目标域名由代理协议携带或经代理内 DNS 解析；仅代理节点主机名允许本地 bootstrap 解析；目标域名不出现在本机 DNS 查询中；WireGuard 隧道内 DNS 必须存在且为 IP 字面量，否则拒绝配置，不生成本地 DNS 或 freedom 回退。

### REQ-PROXY-009

- The system MUST fail closed when the engine is unavailable, configuration validation fails, authentication fails, or the upstream node cannot be reached.
- Inputs: 引擎状态、配置检查结果、出站错误、连接超时。
- Outputs: 浏览器网络失败；不得回退直连、系统代理、其他节点或 HTTP/SOCKS 降级；UI 显示稳定错误类。

### REQ-PROXY-010

- The system MUST pass `--disable-non-proxied-udp` by default for every proxied profile and MUST require explicit advanced confirmation with an audit event to omit it.
- Inputs: WebRTC 策略、档案配置、审计上下文。
- Outputs: 默认阻止非代理 UDP；关闭只能通过指纹规格 `webrtc.disable_non_proxied_udp=false` 的高级确认写入，并记录审计动作 `proxy.non_proxied_udp.disable`、操作者、时间和档案；UI 持续显示风险标记；Hysteria2 的 QUIC 仅用于引擎到节点，不改变浏览器非代理 UDP 策略。

### REQ-PROXY-011

- The system MUST prevent IPv6 direct egress when the selected protocol or node cannot proxy IPv6, and MUST surface the resulting limitation.
- Inputs: 协议能力、节点地址族、宿主或容器 IPv6 配置、内核能力清单的 `network_controls.ipv6_mode`、连接尝试。
- Outputs: IPv6 被代理或被阻止；不出现绕过代理的 IPv6 直连；清单声明 `ipv6_mode=unsupported` 时代理档案拒绝启动并返回 `KERNEL_UNSUPPORTED`，不得静默放行；限制在 UI 与测试报告中可见。

### REQ-PROXY-012

- The system MUST disable DNS-over-HTTPS bypass for proxied profiles and MUST verify the effective configuration during startup or acceptance testing.
- Inputs: 内核能力清单的 `network_controls.doh_mode`、浏览器策略、测试探针。
- Outputs: 指纹规格 `network.doh_mode=disabled` 且内核能力清单声明对应控制；清单声明 `doh_mode=unsupported` 时代理档案拒绝启动并返回 `KERNEL_UNSUPPORTED`；DoH 不绕过代理；无法验证生效值时阻止启动并返回稳定错误，不得静默忽略。

### REQ-PROXY-013

- The system MUST provide a proxy check through temporary inbound, route and outbound resources in the shared engine, then remove those resources; it MUST NOT start a second engine for the check.
- Inputs: 代理配置、共享引擎管理 API、可选回显端点、超时。
- Outputs: 成功/失败、延迟、可选出口 IP 与地区、稳定错误类；节点与网络失败必须用 HTTP 200 的 `ProxyCheck` 表达（`ok=false`，具体错误在 `error_code`），事件 `proxy.check.completed` 携带同一 `error_code`，不得使用未声明的 502；只有认证、授权、校验、存储、引擎未就绪等控制面失败使用 HTTP 错误响应。检查使用独立 `chk:<check_id>:out` 临时出站和独立 `chk:<check_id>:rule`，不写入 `proxy_outbounds`、不改变 `ref_count`；检查不记录凭据、UUID、完整节点 URL 或目标 URL；临时资源在成功或失败后都被删除。

### REQ-PROXY-014

- The system MUST map engine and upstream failures to stable error classes, including configuration invalid, engine unavailable, authentication failure, TLS/REALITY failure, DNS failure, connection refused, timeout, and protocol unsupported.
- Inputs: 配置检查、引擎退出、出站错误、超时。
- Outputs: 稳定错误码与可读消息：`PROXY_CONFIG_INVALID`、`PROXY_ENGINE_*`、`PROXY_AUTH_FAILED`、`PROXY_TLS_FAILED`、`PROXY_DNS_FAILED`、`PROXY_CONNECTION_REFUSED`、`PROXY_TIMEOUT`、`UNSUPPORTED_PROTOCOL`；UI 可区分配置、引擎、节点和目标站点问题；不泄露秘密。

### REQ-PROXY-015

- The system MUST support importing a single share link for `vless://`, `vmess://`, `ss://`, `trojan://`, and `hysteria2://` without fetching the network or executing code.
- Inputs: 分享链接文本、当前引擎能力清单。
- Outputs: 通过解析和校验的代理草稿；不支持的协议、传输或参数返回具体错误；导入不自动保存或启动。

### REQ-PROXY-016

- The system MUST return a redacted proxy view to UI and API clients, exposing non-secret options but only `credential_set` booleans and credential field names for secrets.
- Inputs: 已保存代理配置、请求者权限。
- Outputs: 无 UUID、密码、私钥、完整分享链接或节点 URL 中的敏感查询参数；编辑时可替换凭据但不能读回旧值。

### REQ-PROXY-017

- The system MUST bind the Xray management API and every runtime HTTP inbound to `127.0.0.1` only and allocate a per-runtime port once, persisting it for the lifetime of the runtime. On the server host the management API MUST be reachable only from the control-plane identity, using an LSM/firewall boundary scoped to the control-plane UID or an equivalent separate network boundary; in an unsandboxed server deployment the kernel MUST NOT be able to reach it. The desktop host has no non-root UID-scoped firewall primitive, so it MUST bind only to loopback and prove via an M0 page-originated probe that the page cannot reach the management API, with the same-UID residual risk recorded in `SECURITY.md` T-15/T-22/T-27.
- Inputs: 端口分配、运行时生命周期、本机进程、管理 API、控制面身份、宿主类型（桌面/服务端）、沙箱模式。
- Outputs: 非回环地址无法连接；页面发起的回环探测无法访问管理 API；运行时结束后入站端口关闭；管理 API 不暴露到容器外。

### REQ-PROXY-018

- The system MUST treat direct mode as an explicit risky configuration and MUST NOT create a runtime inbound/outbound for a direct profile.
- Inputs: 档案代理模式、启动请求、宿主类型（桌面/服务端）、`MIEN_ALLOW_DIRECT_EGRESS`、审计上下文。
- Outputs: 直连档案不创建入站/出站；UI 与 API 标记 `direct=true`、降低保证（`reduced_assurance=true`）和风险说明；桌面端允许直连但必须展示降低保证；服务端必须在 `MIEN_ALLOW_DIRECT_EGRESS=1` 时才能启动并写入审计事件，否则返回 `DIRECT_EGRESS_DISABLED`（403）并拒绝启动；T-18 的远程 DNS 与失败关闭保证只适用于代理模式，不得对直连模式声称具备；直连模式仍应用内核支持的 `webrtc`/`network` 标志（至少禁用非代理 UDP、禁用 DoH、阻止 IPv6），无法应用时拒绝启动或记录为已声明限制；不得把直连伪装成代理。

### REQ-PROXY-019

- The system MUST capture engine logs with redaction and rotation, and MUST NOT log full configuration, credentials, share links, or target URLs.
- Inputs: 引擎 stdout/stderr、错误、连接事件。
- Outputs: 结构化或行式日志；单文件上限 10 MB，最多 5 个文件；敏感值不出现；可按档案诊断协议与连接错误。

### REQ-PROXY-020

- The system MUST publish a protocol engine capability manifest containing version, platform, release-asset `sha256`, license, a `protocols` object map (`protocols.<name> = {tier, transports, tls_modes, methods?, plugins?, obfuscation?, notes?}` with `tier` in `A|B|basic` and bound to the protocol name), connection-reuse support, and known limitations, and MUST use it to drive UI and validation.
- Inputs: 引擎安装包、构建清单、测试结果、引擎安装记录。
- Outputs: 机器可读清单；清单 `sha256` 只表示发布资产，安装记录另存 `asset_sha256` 与 `binary_sha256`；Tier A/B 与 basic 分类由清单数据驱动；UI 不展示未声明能力；协议组合按清单拒绝或标注；升级前可比对差异。

### REQ-PROXY-021

- The system MUST share an outbound and its connection pool among running profiles with the same stable `outbound_key`; profiles with different credentials, transport, TLS/REALITY or mux settings MUST NOT share.
- Inputs: 代理配置、凭据引用、传输设置、mux 设置、运行档案。
- Outputs: 每个 key 一个出站；`ref_count` 只统计非终态运行时（`starting`/`running`/`stopping`）；停止或失败 teardown 必须先清空该运行时的 `proxy_outbound_key`、`proxy_inbound_tag`、`proxy_rule_tag`、`proxy_port`，再递减 `ref_count`，且 teardown 幂等、只释放一次；最后一个引用释放时删除出站；复用能力由引擎清单声明。

### REQ-PROXY-022

- The system MUST fail closed when the shared engine crashes, the management API is unavailable, or rebuilding the running configuration fails; it MUST restart with bounded backoff and recreate inbounds on their original `proxy_port`.
- Inputs: 引擎 PID、API 健康、运行档案状态、原端口。
- Outputs: 所有运行档案网络失败；全局 `proxy.engine.failed` 事件；重建成功产生 `proxy.engine.rebuilt`；重建失败先删除受影响运行时的入站、路由规则并释放出站引用和配额/锁，再把运行时标记为 `failed`，不得存在终态但仍有活内核或残留代理资源。

### REQ-PROXY-023

- The system MUST bind the Xray management API to loopback only, serialize mutations, make add/remove idempotent, and reconcile outbound refcounts after restart.
- Inputs: 管理 API 地址、并发启动/停止请求、引擎重启、`runtimes` 与 `proxy_outbounds` 持久化状态。
- Outputs: 无重复出站、无孤儿入站或规则、引用计数按非终态运行时可重建；控制面重启时能归属且资源完整的 `running` 行保持 `running`，无主的 `starting`/`running`/`stopping` 行先终止可归属内核与代理资源，teardown 成功后转 `stopped`；共享引擎重建失败时先转 `stopping`，teardown 完成后才转 `failed`；teardown 失败保留 `stopping` 与 `last_error`；不一致返回 `PROXY_OUTBOUND_REFCOUNT_MISMATCH`；API 不暴露到非回环地址。

### REQ-PROXY-024

- The system MUST identify every route rule by a stable `ruleTag`, add rules incrementally with `shouldAppend=true`, and remove rules only by `ruleTag`; it MUST NOT emit `routing.final`, `routing.script` or any other whole-routing override.
- Inputs: 运行档案标识、Xray `AddRule`/`RemoveRule`、当前路由状态。
- Outputs: 每条规则带 `rt:<runtime_id>:rule`；添加规则不清空或重排已有规则；删除只移除目标 tag；未匹配流量保持断网。

### REQ-PROXY-025

- The system MUST declare the blackhole (`block`) outbound as the first outbound at engine startup and MUST assert this ordering and the absence of `routing.script` before accepting any runtime; assertion failure MUST refuse service instead of falling back to direct.
- Inputs: 启动引导配置、出站列表、引擎启动结果。
- Outputs: 引导配置只包含 block 作为首出站和 API 入站，且不含 `routing.final`、`routing.script` 或 `routing.balancers`；启动断言通过后才服务；断言失败返回稳定引擎错误且不创建任何运行档案资源。

## Verification

### VER-PROXY-001

- Method: 对 Tier A、Tier B 与 basic 的每种协议启动受控节点，通过共享引擎创建入站/出站并通过浏览器访问探针页。
- covers: `REQ-PROXY-001`
- Pass condition: Tier A 全部通过；Hysteria2/WireGuard 按清单通过或明确标注；HTTP/HTTPS/SOCKS5 可用但不计入 A/B 门禁；TUIC 被拒绝；无静默降级。

### VER-PROXY-002

- Method: 使用合法配置、未知字段、错误端口、缺失凭据、非法传输组合和超长字段执行 schema 校验。
- covers: `REQ-PROXY-002`
- Pass condition: 合法配置通过；非法输入返回字段级错误；不产生入站、出站或规则。

### VER-PROXY-003

- Method: 启动两个使用不同协议的档案，再启动第三个使用相同节点/凭据的档案，检查进程、端口、出站复用和停止顺序。
- covers: `REQ-PROXY-003`
- Pass condition: 全宿主只有一个 Xray 引擎；每个运行档案有独立入站/端口/规则；相同 key 的档案共享出站；停止后入站和规则释放、引用归零时出站删除；沙箱校验失败时不创建任何代理资源。

### VER-PROXY-004

- Method: 分别篡改发布资产摘要、篡改解包后的可执行文件、删除可执行文件、用不相符的 `binary_sha256` 替换安装记录。
- covers: `REQ-PROXY-004`
- Pass condition: 安装阶段校验发布资产摘要（清单 `sha256` 与安装记录 `asset_sha256` 一致）；运行时只校验 `binary_sha256`；文件缺失返回 `PROXY_ENGINE_NOT_FOUND`，摘要不符返回 `PROXY_ENGINE_CHECKSUM_MISMATCH`；均无入站、出站或规则残留。

### VER-PROXY-005

- Method: 检查由 Xray API 创建的入站、出站和规则，并尝试构造直连或未匹配流量。
- covers: `REQ-PROXY-005`
- Pass condition: 每个运行档案有一个 HTTP 入站和一条带稳定 `ruleTag` 的路由规则；默认出站为 block；无直连回退；同一 key 只有一个出站。

### VER-PROXY-006

- Method: 启动后扫描 argv、日志、事件、API 响应和持久化文件，检查凭据是否只经回环 API 传递。
- covers: `REQ-PROXY-006`
- Pass condition: 凭据不出现在 argv、日志、事件、API 响应或持久化引擎配置；重启可重新生成；管理 API 只绑定控制面回环，并按宿主类型由控制面 UID 独占的 LSM/防火墙策略或独立网络边界限制。

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
- Pass condition: 节点与网络失败以 HTTP 200 `ProxyCheck` 返回，`ok=false` 且 `error_code` 为具体错误；不出现 502；控制面失败才使用 HTTP 错误响应；延迟和出口 IP 仅在可用时返回；日志不含秘密；临时入站/出站/规则被删除。

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
- Pass condition: 非回环连接被拒绝；运行时结束后入站端口关闭；管理 API 不可从非回环访问；服务端无沙箱模式下内核身份不可达，服务端沙箱模式下页面发起的回环探测不可达；桌面端记录同 UID 残余风险并以页面探测为补偿控制。

### VER-PROXY-018

- Method: 分别在桌面与服务端创建直连档案并启动，检查进程、UI、API、环境变量与审计；再在服务端不设置 `MIEN_ALLOW_DIRECT_EGRESS` 时重试。
- covers: `REQ-PROXY-018`
- Pass condition: 不创建运行档案入站/出站；桌面端允许直连并显示降低保证；服务端未设置开关时拒绝启动，设置后启动并写入审计；`direct=true`、`reduced_assurance=true` 与风险说明可见；不声称具备 T-18 代理保护；无代理成功伪装。

### VER-PROXY-019

- Method: 触发协议错误、认证失败和长时间运行，检查日志内容、权限与轮转。
- covers: `REQ-PROXY-019`
- Pass condition: 日志轮转符合上限；无配置、凭据、分享链接或目标 URL；可定位错误类。

### VER-PROXY-020

- Method: 加载引擎清单，渲染代理编辑器，并尝试提交未声明协议或传输。
- covers: `REQ-PROXY-020`
- Pass condition: 清单字段完整，`protocols` 为对象映射且 `tier` 取值为 `A|B|basic`；清单 `sha256` 与安装记录的 `asset_sha256`/`binary_sha256` 语义不混用；UI 只显示声明能力；未声明组合被拒绝；升级差异可比较。

### VER-PROXY-021

- Method: 使用相同节点/凭据启动多个档案，再使用不同凭据启动一个档案；检查 Xray 出站、连接和引用计数。
- covers: `REQ-PROXY-021`
- Pass condition: 相同 key 只创建一个出站并复用；不同凭据创建不同出站；`ref_count` 只计非终态运行时；teardown 先快照四个代理绑定字段，再递减并清空，重复 teardown 不重复释放；最后一个引用释放时删除；日志不泄露凭据。

### VER-PROXY-022

- Method: 运行多个档案时强杀 Xray，观察失败关闭、事件、重启和原端口重建；再模拟重建失败。
- covers: `REQ-PROXY-022`
- Pass condition: 所有运行档案网络失败；产生全局事件；重启后按原端口重建；重建失败时先删除入站/规则、释放出站引用与配额/锁，再标记受影响运行时 `failed`，不得有残留代理资源；无直连回退。

### VER-PROXY-023

- Method: 并发启动/停止档案，重启引擎，检查 Xray API 访问控制、幂等和引用计数对账。
- covers: `REQ-PROXY-023`
- Pass condition: 无重复出站、无孤儿入站/规则；引用计数按非终态运行时重建；控制面重启时能归属且资源完整的 `running` 行保持 `running`，无主的 `starting`/`running`/`stopping` 行先清理资源再转 `stopped`，且该清理必须经过 `stopping` 并发布对应事件；不一致返回 `PROXY_OUTBOUND_REFCOUNT_MISMATCH`；API 仅回环可达，但回环不提供调用者鉴权，服务端必须由控制面 UID 独占的 LSM/防火墙策略或独立网络边界限制，页面发起的回环探测不可达；无沙箱模式还要求内核与管理 API 之间不可路由；桌面端记录同 UID 残余风险。并发操作不产生竞争状态。

### VER-PROXY-024

- Method: 并发添加与删除多个运行档案的路由规则，检查 Xray 路由列表、规则顺序和 tag；模拟删除一个 tag 后再次删除。
- covers: `REQ-PROXY-024`
- Pass condition: 添加使用 `shouldAppend=true`，已有规则不清空或重排；每条规则有稳定 `ruleTag`；删除只移除目标 tag 且幂等；配置中不存在 `routing.final`。

### VER-PROXY-025

- Method: 检查引导配置的出站顺序，并注入非 blackhole 首出站的错误引导配置；尝试在断言失败后启动运行档案。
- covers: `REQ-PROXY-025`
- Pass condition: 正常引导中 block 是首出站；错误引导被启动断言拒绝；断言失败时不服务任何运行档案且不回退直连。

## Related ADRs

- [ADR-0003：SQLite 存储与字段级密钥保护](../../adr/0003-storage-and-secret-protection.md)
- [ADR-0007：每档案独立协议引擎（已被 ADR-0009 取代）](../../adr/0007-protocol-engine-forward-proxy.md)
- [ADR-0009：单实例 Xray 动态协议引擎与连接复用](../../adr/0009-shared-xray-engine.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../XRAY-MAPPING.md`](../../XRAY-MAPPING.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- [`../../contracts/proxy-engine-manifest.schema.json`](../../contracts/proxy-engine-manifest.schema.json)
- <https://github.com/XTLS/Xray-core>
