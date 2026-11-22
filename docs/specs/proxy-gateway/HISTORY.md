# 代理中继主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题在开发启动前建立，绑定 fingerprint-chromium 148 的 `--proxy-server` 与 WebRTC 行为。
- 上游协议支持 HTTP、HTTPS 与 SOCKS5；不承诺代理 UDP。
- 中继与运行时同生命周期，不跨档案复用。

## Replacements / Background

- 选择本地中继而不是扩展或 CDP 认证，是为了覆盖人工浏览和自动化两种场景，并集中处理远程 DNS 与失败关闭。
- 代理检查使用独立短连接，不复用运行中继，避免检查影响浏览器流量。
- 出口 IP 查询依赖可配置回显端点，产品不内置第三方查询服务。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0004-local-proxy-relay.md`](../../adr/0004-local-proxy-relay.md)
