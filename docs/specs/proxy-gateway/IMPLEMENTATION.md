# 代理中继实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 中继、认证注入、远程 DNS 与泄漏防护契约已建立。

## Implementation Coverage

- Requirement coverage: 计划由 `snailfish-core` 的 proxy 模块覆盖 `REQ-PROXY-001` 至 `REQ-PROXY-015`；容器网络策略由 Docker 入口脚本与部署清单提供。
- Verification commands: 尚未建立；实现后使用代理 fixture、DNS canary、WebRTC 探针、IPv6 与失败关闭测试。
- Rollout facts: 首个实现支持 HTTP、HTTPS、SOCKS5 上游；不代理 UDP。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 中继协议适配、认证注入、远程 DNS、失败关闭、IPv6/DoH 防护和代理检查均未实现。
- 需要先验证目标上游代理的认证与 DNS 行为，再冻结错误分类和 fixture。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
