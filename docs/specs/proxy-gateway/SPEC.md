# 代理中继与网络泄漏防护

> 本文件是代理主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: fingerprint-chromium 不支持在 `--proxy-server` 中携带认证凭据，产品需要为每个档案注入上游代理认证，并防止 DNS、WebRTC、IPv6 和 DoH 绕过代理。
- In scope: 每档案本地中继、上游协议适配、认证注入、远程 DNS、失败关闭、泄漏防护、代理检查、日志脱敏、中继生命周期。
- Out of scope: 档案代理字段存储（由 `profile-management` 负责）、指纹字段（由 `fingerprint-engine` 负责）、内核进程监督（由 `browser-runtime` 负责）。

## Terms and Interfaces

- `relay`: 每个运行档案独占的本地 HTTP 代理监听器，绑定 `127.0.0.1` 临时端口。
- `upstream`: 用户配置的 HTTP、HTTPS 或 SOCKS5 代理。
- `remote DNS`: 目标域名由上游代理解析，本机不解析目标域名。
- `fail-closed`: 中继关闭或上游不可用时，浏览器失去网络，而不是回退直连。
- Interface: 内核启动参数 `--proxy-server=http://127.0.0.1:<port>`；代理配置对象 `scheme`、`host`、`port`、`username`、`password`、`dns_mode`；代理检查端点 `POST /api/v1/proxies/check`。

## Requirements

### REQ-PROXY-001

- The system MUST start one relay per running profile, bind it to `127.0.0.1` on an ephemeral port, and close it when the runtime stops or fails to start.
- Inputs: 档案代理配置、运行时启动/停止事件。
- Outputs: 独占的 `relay_port`；中继关闭后端口释放；不得在档案之间复用同一中继。

### REQ-PROXY-002

- The system MUST accept browser HTTP proxy traffic, including `CONNECT` for TLS and absolute-form HTTP requests, and forward it to the configured upstream using the matching protocol.
- Inputs: 浏览器请求、上游协议、上游地址。
- Outputs: 建立连接、转发字节流或返回代理错误；不得把浏览器流量发往直连。

### REQ-PROXY-003

- The system MUST inject upstream credentials inside the relay and MUST NOT place credentials in the kernel argv, logs, events, error messages, or API responses.
- Inputs: 加密存储的用户名与密码、上游协议。
- Outputs: 上游认证成功或明确的认证失败；可读输出只显示“已设置/未设置”；内存中的凭据在不再需要时释放。

### REQ-PROXY-004

- The system MUST resolve proxied destination hostnames through the upstream proxy and MUST NOT perform local DNS resolution for destination hosts.
- Inputs: `CONNECT host:port`、绝对 URI、SOCKS5 域名地址。
- Outputs: 上游收到原始域名；本机不产生目标域名的 DNS 查询；仅上游代理自身主机名可以由本机解析。

### REQ-PROXY-005

- The system MUST fail closed when the relay is unavailable, when upstream authentication fails, or when the upstream connection cannot be established.
- Inputs: 中继状态、上游响应、连接超时。
- Outputs: 浏览器请求失败；不得回退直连、系统代理或其他上游；UI 显示可诊断错误。

### REQ-PROXY-006

- The system MUST pass `--disable-non-proxied-udp` by default for every proxied profile and MUST require an explicit advanced confirmation with audit event to omit it.
- Inputs: WebRTC 策略、档案配置、审计上下文。
- Outputs: 默认阻止非代理 UDP；高级关闭时记录操作者、时间和档案；UI 持续显示风险标记。

### REQ-PROXY-007

- The system MUST disable DNS-over-HTTPS bypass for proxied profiles and MUST verify the effective configuration during startup or acceptance testing.
- Inputs: 内核能力清单、浏览器策略、测试探针。
- Outputs: DoH 不绕过代理；无法禁用或无法验证时必须阻止启动或显示明确限制，不得静默忽略。

### REQ-PROXY-008

- The system MUST prevent IPv6 direct egress when the upstream cannot proxy IPv6, and MUST surface the resulting limitation.
- Inputs: 上游能力、宿主或容器 IPv6 配置、连接尝试。
- Outputs: IPv6 被代理或被阻止；不出现绕过代理的 IPv6 直连；限制在 UI 与测试报告中可见。

### REQ-PROXY-009

- The system MUST provide a proxy check that verifies TCP/TLS connectivity, authentication, latency, and, when an echo endpoint is configured, the exit IP and optional region.
- Inputs: 代理配置、可选回显端点、超时。
- Outputs: 成功/失败、延迟、可选出口 IP 与地区；失败原因分类；检查不记录凭据或完整目标 URL。

### REQ-PROXY-010

- The system MUST map upstream failures to stable error classes, including authentication failure, DNS failure, connection refused, timeout, TLS failure, and protocol mismatch.
- Inputs: 上游响应、套接字错误、超时。
- Outputs: 稳定错误码与可读消息；UI 可区分“代理不可用”和“目标站点不可用”；不泄露上游凭据。

### REQ-PROXY-011

- The system MUST enforce connection and idle timeouts, bound concurrent connections per relay, and clean up half-closed connections without leaking file descriptors.
- Inputs: 并发连接、空闲连接、上游响应速度、资源上限。
- Outputs: 超时连接被关闭；连接数受限；长期运行不出现文件描述符或内存持续增长。

### REQ-PROXY-012

- The system MUST bind the relay to loopback only and MUST keep its lifetime bounded by the runtime; it MUST document the residual risk that same-user local processes can use the loopback port.
- Inputs: 网络命名空间、运行时生命周期、本机进程。
- Outputs: 非回环地址无法连接；运行时结束后端口不可用；安全文档记录同用户残余风险。

### REQ-PROXY-013

- The system MUST redact proxy credentials and destination URLs from relay logs and metrics, retaining only protocol, result, latency, byte counts, and a safe destination identifier.
- Inputs: 连接事件、错误、统计。
- Outputs: 日志与指标不含用户名、密码、完整 URL、查询参数或 Cookie；可按档案和结果聚合。

### REQ-PROXY-014

- The system MUST treat direct mode as an explicit risky configuration and MUST display a persistent warning in profile details and runtime status.
- Inputs: 档案代理模式、启动请求。
- Outputs: 直连档案不启动中继；UI 与 API 标记 `direct=true` 和风险说明；不得把直连伪装成代理。

### REQ-PROXY-015

- The system MUST emit a proxy check completion event with profile, result, latency, and error class, and MUST update the stored last-check summary.
- Inputs: 检查完成、失败或超时。
- Outputs: 事件与数据库摘要一致；UI 无需轮询即可更新状态；不包含敏感值。

## Verification

### VER-PROXY-001

- Method: 启动两个档案，读取各自中继端口，停止其中一个后再次连接。
- covers: `REQ-PROXY-001`
- Pass condition: 两个端口不同且只绑定回环；停止后端口拒绝连接；另一档案不受影响。

### VER-PROXY-002

- Method: 通过每个档案访问 HTTP、HTTPS 和带认证的上游代理，检查转发路径。
- covers: `REQ-PROXY-002`
- Pass condition: `CONNECT` 与绝对 URI 均成功转发；上游收到请求；无直连流量。

### VER-PROXY-003

- Method: 使用错误与正确凭据，扫描进程参数、日志、事件和 API 响应。
- covers: `REQ-PROXY-003`
- Pass condition: 正确凭据成功；错误凭据返回认证失败；凭据不出现在任何可读输出中。

### VER-PROXY-004

- Method: 使用 DNS canary 域名访问目标，监控本机 DNS 查询和上游日志。
- covers: `REQ-PROXY-004`
- Pass condition: 本机没有目标域名查询；上游收到域名并完成解析；上游代理主机名解析不受影响。

### VER-PROXY-005

- Method: 在页面加载中关闭中继、使用错误密码、断开上游网络。
- covers: `REQ-PROXY-005`
- Pass condition: 所有场景网络失败；无直连成功；UI 显示对应错误类。

### VER-PROXY-006

- Method: 默认启动并读取 argv，再通过高级开关关闭保护并检查审计。
- covers: `REQ-PROXY-006`
- Pass condition: 默认包含 `--disable-non-proxied-udp`；高级关闭需确认且有审计事件；风险标记持续显示。

### VER-PROXY-007

- Method: 在探针中检查 DoH 状态和 DNS 路径，尝试触发安全 DNS。
- covers: `REQ-PROXY-007`
- Pass condition: DoH 未绕过代理；无法验证时启动被阻止或明确标记限制。

### VER-PROXY-008

- Method: 在启用 IPv6 的容器中访问 IPv6-only 与双栈目标，抓取直连尝试。
- covers: `REQ-PROXY-008`
- Pass condition: IPv6 流量被代理或被阻止；无直连 IPv6 连接；限制在 UI 中可见。

### VER-PROXY-009

- Method: 对可用、认证失败、超时和不可达代理执行检查，配置与不配置回显端点。
- covers: `REQ-PROXY-009`
- Pass condition: 检查结果与错误类正确；延迟和出口 IP 仅在可用时返回；日志不含凭据。

### VER-PROXY-010

- Method: 模拟认证失败、DNS 失败、拒绝连接、超时、TLS 失败和协议错误。
- covers: `REQ-PROXY-010`
- Pass condition: 每种失败返回不同稳定错误码；消息可区分代理与目标问题；无凭据泄露。

### VER-PROXY-011

- Method: 执行高并发连接、空闲连接和半关闭连接测试，监控文件描述符与内存。
- covers: `REQ-PROXY-011`
- Pass condition: 超时和上限生效；连接被清理；长跑后资源无持续增长。

### VER-PROXY-012

- Method: 从非回环地址连接中继，并在运行时结束后重试；检查安全文档。
- covers: `REQ-PROXY-012`
- Pass condition: 非回环连接被拒绝；运行时结束后端口关闭；文档明确记录同用户残余风险。

### VER-PROXY-013

- Method: 触发成功、失败和高流量连接，扫描日志与指标。
- covers: `REQ-PROXY-013`
- Pass condition: 无用户名、密码、完整 URL 或 Cookie；保留协议、结果、延迟和字节统计。

### VER-PROXY-014

- Method: 创建直连档案并启动，检查 UI、API 和运行时状态。
- covers: `REQ-PROXY-014`
- Pass condition: 不启动中继；`direct=true` 与风险说明可见；无代理成功伪装。

### VER-PROXY-015

- Method: 完成一次成功检查和一次失败检查，订阅事件流并读取数据库摘要。
- covers: `REQ-PROXY-015`
- Pass condition: 事件按序到达；摘要与事件一致；UI 无需轮询更新；事件不含敏感值。

## Related ADRs

- [ADR-0003：SQLite 存储与字段级密钥保护](../../adr/0003-storage-and-secret-protection.md)
- [ADR-0004：每档案本地代理中继](../../adr/0004-local-proxy-relay.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
