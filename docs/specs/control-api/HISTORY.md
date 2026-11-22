# 控制面 API 主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题在开发启动前建立，API 版本从 `/api/v1` 开始。
- 桌面与服务端共享命令语义，但认证模型不同；桌面默认本地单用户，服务端使用会话与 Token。
- 事件标识单调递增，允许客户端断线后按标识补齐。

## Replacements / Background

- 选择 REST + WebSocket 而不是 GraphQL，是为了降低服务端复杂度、便于生成客户端和调试。
- 选择会话 Cookie 与 Bearer Token 并存，是为了同时覆盖浏览器 UI 和自动化客户端。
- WebSocket 不使用 URL 长效 Token，避免令牌进入日志与历史记录；使用会话 Cookie 或一次性票据。
- 控制面不代理 CDP 或 noVNC，避免把高权限入口变成 API 的可复用凭据。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0001-shared-core-two-hosts.md`](../../adr/0001-shared-core-two-hosts.md)
