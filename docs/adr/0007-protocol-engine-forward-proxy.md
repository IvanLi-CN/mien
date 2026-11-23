# ADR-0007：协议引擎承载正向代理

- 状态：已被 [ADR-0009](0009-shared-xray-engine.md) 取代
- 决策范围：VLESS、VMess、Shadowsocks、Trojan、Hysteria2 等正向代理协议、认证、DNS 与进程边界
- 取代：[ADR-0004](0004-local-proxy-relay.md)

## 背景

产品最初把代理边界设计成自建本地 HTTP 中继：Chromium 只支持 HTTP、HTTPS、SOCKS 代理，且 `--proxy-server` 不能携带认证凭据，中继负责注入上游认证。该方案足以覆盖 HTTP/SOCKS 代理，但无法覆盖 VLESS、VMess、Shadowsocks、Trojan、Hysteria2、TUIC 等主流正向代理协议。自行实现这些协议意味着长期维护 TLS、REALITY、WebSocket、gRPC、QUIC、AEAD、mux 和多种传输组合，安全与兼容成本远高于产品核心价值。

产品需要一种不修改 Chromium、不自行实现密码学协议、能够统一处理简单代理与高级协议、并且仍然保证失败关闭和凭据不进入浏览器进程的方案。

## 决策

- 使用 **sing-box** 作为正向代理协议引擎，作为独立可执行文件随产品分发，不修改其源码。
- 每个运行档案启动一个独占的 sing-box 进程，生命周期由代理网关监督。
- sing-box 暴露一个只绑定 `127.0.0.1` 的 `mixed` 入站，同时接受 HTTP 与 SOCKS；Chromium 只接收 `--proxy-server=http://127.0.0.1:<proxy_port>`。
- 代理网关根据档案的代理配置生成 sing-box 配置：一个 mixed 入站、一个协议出站、路由最终指向该出站，不生成直连回退。
- 协议与凭据由 sing-box 处理；Chromium 不接触 VLESS UUID、VMess 用户 ID、Shadowsocks 密码、Trojan 密码、Hysteria2 密码或 TLS/REALITY 参数。
- 首个发布必须支持：VLESS、VMess、Shadowsocks、Trojan。Hysteria2、TUIC、WireGuard 作为受支持但独立标注的协议矩阵项；其他引擎能力按能力清单暴露。
- 支持从单条分享链接导入（`vless://`、`vmess://`、`ss://`、`trojan://`、`hysteria2://`）；订阅 URL 导入与定时刷新属于后续范围。
- 代理网关在启动引擎前执行配置 schema 校验和 `sing-box check`；配置写入 0600 文件，成功启动后删除，崩溃重启时从加密数据库重新生成。
- 引擎配置只包含一个代理出站，不启用 Clash API 或远程控制入口；目标域名由代理协议的域名地址或经代理的 DNS 解析，只有代理节点主机名允许本地 bootstrap 解析。
- 引擎二进制固定版本、SHA-256 校验、协议能力清单；升级走独立准入，不静默替换运行中档案。
- sing-box 以 GPL-3.0-or-later 分发，产品将其作为独立进程使用并遵守许可：保留许可证、提供对应源码或书面要约、不修改二进制、不使用其名称暗示关联。发布前必须完成法律评审；若评审不接受，后继决策改用 Xray-core（MPL-2.0）并缩减协议矩阵。

## 备选方案

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| 在 Rust 核心中自行实现协议 | 单进程、无外部依赖 | 协议与密码学维护成本极高；REALITY/QUIC/mux 组合复杂 | 不采用 |
| 使用 Xray-core | MPL-2.0；VLESS/VMess/SS/Trojan 成熟 | 不支持 Hysteria2/TUIC；协议矩阵较窄 | 备选 |
| 使用 sing-box | 协议覆盖广；单二进制；跨平台；配置成熟 | GPL-3.0；额外进程；配置包含凭据 | 采用 |
| 保留自建中继并转发到 sing-box | 保留原有指标与限流 | 多一跳、多一个进程；中继仍需处理协议边界 | 不采用 |
| 只支持 HTTP/SOCKS | 实现简单 | 不满足 VLESS/VMess/SS 需求 | 不满足 |

## 后果

- 每个代理档案多一个 sing-box 进程，增加内存和启动时间；容量测试必须覆盖 10 个并发档案。
- 代理协议、TLS/REALITY/WS/gRPC/QUIC 和认证由上游引擎维护，产品只需维护配置映射、能力清单和生命周期。
- 引擎配置包含 UUID、密码、私钥等敏感值；必须使用 0600 文件、加密数据库、日志脱敏和启动后删除配置。
- 引擎故障时浏览器必须失败关闭；不得生成直连回退，也不得切换到其他代理。
- 代理节点主机名可能需要本地 bootstrap 解析，这会暴露节点域名；该限制必须在 UI 与安全文档中说明。目标域名不得本地解析。
- GPL 合规成为发布门禁；镜像和应用必须包含许可证、源码链接和书面要约。
- 订阅、节点测速和多节点切换不在首个发布范围；单条分享链接导入必须可用。
- Hysteria2/TUIC 使用 QUIC/UDP 连接代理服务器，但 Chromium 仍通过本地 HTTP 代理发送 TCP 流量；WebRTC 的非代理 UDP 仍默认阻止。

## 证据

- sing-box 官方发行包提供 `darwin-arm64`、`linux-amd64` 等平台二进制，并支持 VLESS、VMess、Shadowsocks、Trojan、Hysteria2、TUIC、WireGuard 等出站。
- sing-box 许可证为 GPL-3.0-or-later，并附带名称使用限制。
- fingerprint-chromium 的 `--proxy-server` 不支持认证凭据，且内核不应接触上游秘密。
- 产品需要同时覆盖简单 HTTP/SOCKS 与高级协议，单一 mixed 入站可以统一浏览器侧配置。

## 参考

- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- [`../specs/proxy-gateway/SPEC.md`](../specs/proxy-gateway/SPEC.md)
- [`../contracts/proxy-engine-manifest.schema.json`](../contracts/proxy-engine-manifest.schema.json)
- <https://github.com/SagerNet/sing-box>
- <https://github.com/SagerNet/sing-box/blob/dev/LICENSE>
