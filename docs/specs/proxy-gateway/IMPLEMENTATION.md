# 正向代理实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 协议引擎、类型化代理配置、分享链接与失败关闭契约已建立。

## Implementation Coverage

- Requirement coverage: 计划由 `mien-core` 的 proxy 模块和固定 sing-box 引擎覆盖 `REQ-PROXY-001` 至 `REQ-PROXY-020`；协议能力由引擎清单驱动。
- Verification commands: 尚未建立；实现后使用受控 VLESS/VMess/Shadowsocks/Trojan/Hysteria2 节点、DNS canary、引擎故障注入、分享链接 fixture 和凭据扫描。
- Rollout facts: Tier A 为 VLESS、VMess、Shadowsocks、Trojan；Tier B 为 Hysteria2、TUIC、WireGuard；订阅属于后续范围。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 类型化代理配置、协议引擎配置生成、生命周期、分享链接解析、远程 DNS、失败关闭与代理检查均未实现。
- 需要先完成 sing-box 能力清单、GPL 法律评审和目标容器 QUIC/UDP 可行性验证，再冻结协议矩阵。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
