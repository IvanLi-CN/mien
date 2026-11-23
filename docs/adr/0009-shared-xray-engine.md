# ADR-0009：单实例 Xray 动态协议引擎与连接复用

- 状态：已接受
- 决策范围：正向代理协议引擎的进程数量、运行时配置变更、连接复用、故障域与许可
- 取代：[ADR-0007](0007-protocol-engine-forward-proxy.md)

## 背景

ADR-0007 选择每个运行档案启动一个独占的 sing-box 进程。该模型隔离清晰，但它把同一代理节点的连接池拆成多份：两个档案即使使用完全相同的节点和凭据，也会各自建立连接。对于有并发连接上限或共享风控的正向代理节点，这会直接放大连接数和风控暴露面。

产品的网络层约束因此变为：

- 每个宿主最多一个常驻正向代理引擎进程。
- 档案可以在运行时增加、删除和停止，不能因为一个档案启停而重启整个引擎。
- 使用相同节点、凭据、传输和 TLS 配置的档案必须能够复用同一个出站与连接池。
- 引擎配置不能回退到直连；任一档案的代理失败只影响该档案，但引擎进程故障会同时影响所有运行档案，必须明确记录。

sing-box 官方二进制不能满足前两项：它的 Clash API 只支持查询、切换已声明的 Selector、切换 mode；`PUT /configs` 是空实现，`PATCH /configs` 只改 mode；`SIGHUP` 会关闭整个实例再重新创建，期间所有档案断网且连接池丢失。它没有运行时增删入站、出站或路由规则的公开 API。

Xray-core 提供 `HandlerService` 的 `AddInbound`、`RemoveInbound`、`AddOutbound`、`RemoveOutbound`，以及 `RoutingService` 的 `AddRule`、`RemoveRule`；Xray-core `v26.11.9` 已包含 Hysteria2。它缺少 TUIC，因此 TUIC 继续留在后续范围。

## 决策

- 每个宿主只运行一个受监督的 Xray-core 进程，作为共享正向代理引擎。
- Xray 的 gRPC 管理 API 只绑定 `127.0.0.1` 的随机端口，不暴露到局域网、容器外或公网；管理 API 与浏览器入站使用不同端口。
- 每个运行档案分配一个固定的回环 HTTP 入站端口，并创建一条以该入站为条件的路由规则；Chromium 只接收 `--proxy-server=http://127.0.0.1:<proxy_port>`。
- 出站按 `outbound_key = hash(protocol, server, port, credential_ref, transport, TLS/REALITY, mux_settings)` 共享。相同 key 的档案共享一个出站；不同凭据或不同传输不共享。
- 每个共享出站维护引用计数。第一个使用它的运行档案创建出站，最后一个停止的档案删除出站；路由规则和入站则按运行档案创建和删除。
- 在协议能力允许时启用 Xray mux；Hysteria2 使用其 QUIC 连接复用。连接复用能力由引擎能力清单声明，不能对不支持的协议假装复用。
- Xray 的默认路由出站必须是 block；不得生成 direct 出站、系统代理回退或其他节点的静默降级。
- 代理网关负责 API 调用的串行化、幂等、引用计数、失败回滚和启动时对账。引擎重启后，网关按原始 `proxy_port` 重建入站、出站和路由规则。
- 引擎进程故障时，所有运行档案立即失去网络并产生 `proxy.engine.failed`；网关按有界退避重启引擎。重建成功则恢复运行档案的网络；重建失败则把受影响运行时标记为 `failed`，不自动直连。
- 固定 Xray-core `v26.11.9` 或经准入的后继补丁版本，校验 SHA-256，并维护协议能力清单。TUIC 不在 v1；Hysteria2、VLESS、VMess、Shadowsocks、Trojan、HTTP/SOCKS 按清单支持。
- Xray-core 以 MPL-2.0 分发；发布物保留许可证、版权与来源信息，不修改二进制。

## 备选方案

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| 每运行档案一个 sing-box | 故障与凭据隔离清晰；实现简单 | 多份连接池；触发节点连接限制与共享风控；无法单实例动态 CRUD | 不采用 |
| 单实例 sing-box + `SIGHUP` 重载 | 保留 sing-box 协议覆盖 | 每次档案变更全量重启；连接池丢失；所有档案短暂断网；无动态 CRUD | 不采用 |
| 每个代理节点一个 sing-box | 单节点连接集中 | 仍可能有多个引擎；档案切换节点需要重启该节点引擎；动态能力不足 | 不采用 |
| 单实例 Xray + 动态 API | 单进程、动态入站/出站/规则、连接复用、MPL-2.0 | 共享故障域；API 是高权限控制面；TUIC 缺失；实现复杂度上升 | 采用 |
| 自研 Go 控制层嵌入 sing-box | 可调用内部 manager | 路由规则无公开 CRUD；偏离固定二进制策略；维护成本高 | 不采用 |

## 后果

- 一个引擎进程承载多个档案，连接池按 `outbound_key` 共享；节点连接数和风控暴露面显著低于每档案一个引擎。
- 引擎进程是共享单点。引擎崩溃、API 不可用或重建失败会影响所有运行档案；必须通过失败关闭、有界重启、对账和明确 UI 状态处理。
- 代理凭据集中在一个引擎进程的受保护内存中；不再依赖每档案临时配置文件的短生命周期。API 只绑定回环，日志与错误继续脱敏。
- 代理网关的实现复杂度上升：需要管理出站引用计数、入站/规则标签、端口复用、API 幂等、并发锁和引擎重启后的全量重建。
- 共享出站会让同一节点看到来自多个档案复用连接的流量，这是连接复用的目标；如果某个档案必须避免与其它档案共享上游连接，需要为该档案使用不同的节点或凭据，使其 `outbound_key` 不同。
- TUIC 退出 v1；Xray 不支持 TUIC。Hysteria2 保留，但需要锁定包含其实现并经过准入的 Xray 版本。
- Xray 的 MPL-2.0 降低了 GPL 分发与法律评审风险；仍需保留许可证和来源信息，并完成发布物合规检查。

## 证据

- Xray-core `app/proxyman/command/command.proto` 定义 `AddInbound`、`RemoveInbound`、`AddOutbound`、`RemoveOutbound`。
- Xray-core `app/router/command/command.proto` 定义 `AddRule`、`RemoveRule`。
- Xray-core `v26.11.9` 的源码树包含 `proxy/hysteria` 与 Hysteria2 配置映射；不包含 TUIC。
- sing-box `v1.14.0` 的 `experimental/clashapi/configs.go` 中 `PUT /configs` 为空实现，`PATCH /configs` 只设置 mode；`cmd/sing-box/cmd_run.go` 的 `SIGHUP` 会关闭并重建实例。
- 产品需求要求失败关闭、凭据不进入浏览器、目标域名不本地解析；共享引擎不改变这些边界。

## 参考

- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- [`../DESIGN.md`](../DESIGN.md)
- [`../specs/proxy-gateway/SPEC.md`](../specs/proxy-gateway/SPEC.md)
- [`0007-protocol-engine-forward-proxy.md`](0007-protocol-engine-forward-proxy.md)
- <https://github.com/XTLS/Xray-core>
