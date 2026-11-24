# 正向代理主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题最初定义自建本地中继，覆盖 HTTP、HTTPS、SOCKS5 与认证注入；该数据面方案已由 ADR-0007 取代。
- 随后 ADR-0007 引入每档案独立 sing-box 协议引擎；ADR-0009 因其无法满足单实例动态增删和连接复用，改为每宿主一个共享 Xray 引擎。
- 当前方案为单实例 Xray 引擎、每运行档案回环 HTTP 入站与路由、按 `outbound_key` 共享出站和连接池。
- 协议矩阵按引擎版本和能力清单演进；Tier A 协议参与发布门禁，Tier B 协议按独立测试标注，HTTP/HTTPS/SOCKS5 为 basic tier、不参与 A/B 门禁。

## Replacements / Background

- 自建中继无法覆盖 VLESS、VMess、Shadowsocks、Trojan、Hysteria2 等协议；自行实现协议与密码学的维护成本不可接受。
- sing-box 最初因协议覆盖被选为统一引擎；其官方二进制不支持运行时增删入站/出站/路由，`SIGHUP` 又是全量重建，无法满足代理节点连接限制与共享风控要求。
- Xray-core 因此成为当前引擎；它提供 HandlerService/RoutingService 动态 API，使用 MPL-2.0，v26.11.9 已包含 Hysteria2 与 WireGuard 出站，但不支持 TUIC。
- 代理配置从“用户名/密码 + 协议字段”演进为类型化配置与能力清单校验；导出（含加密导出）都不包含代理凭据，只保留凭据字段名，导入后需重新提供凭据。
- 订阅 URL 导入与多节点调度被明确排除在首个发布范围之外，避免节点列表、刷新和切换语义挤压核心代理路径。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0004-local-proxy-relay.md`](../../adr/0004-local-proxy-relay.md)
- [`../../adr/0007-protocol-engine-forward-proxy.md`](../../adr/0007-protocol-engine-forward-proxy.md)
- [`../../adr/0009-shared-xray-engine.md`](../../adr/0009-shared-xray-engine.md)
