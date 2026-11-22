# 正向代理主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题最初定义自建本地中继，覆盖 HTTP、HTTPS、SOCKS5 与认证注入；该数据面方案已由 ADR-0007 取代。
- 当前方案为每档案独立 sing-box 协议引擎，通过 loopback mixed 入站向 Chromium 提供代理入口。
- 协议矩阵按引擎版本和能力清单演进；Tier A 协议参与发布门禁，Tier B 协议按独立测试标注。

## Replacements / Background

- 自建中继无法覆盖 VLESS、VMess、Shadowsocks、Trojan、Hysteria2 等协议；自行实现协议与密码学的维护成本不可接受。
- sing-box 被选为统一协议引擎，以独立进程运行，不修改源码；GPL-3.0 合规与法律评审成为发布门禁。
- 代理配置从“用户名/密码 + 协议字段”演进为类型化配置与能力清单校验；默认导出不包含凭据，加密导出可完整迁移。
- 订阅 URL 导入与多节点调度被明确排除在首个发布范围之外，避免节点列表、刷新和切换语义挤压核心代理路径。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0004-local-proxy-relay.md`](../../adr/0004-local-proxy-relay.md)
- [`../../adr/0007-protocol-engine-forward-proxy.md`](../../adr/0007-protocol-engine-forward-proxy.md)
