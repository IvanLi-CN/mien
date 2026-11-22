# 控制面 API 实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 服务端认证、Token、事件与控制面契约已建立。

## Implementation Coverage

- Requirement coverage: 计划由 `snailfish-server` 的路由、认证、WebSocket 与静态资源模块覆盖 `REQ-API-001` 至 `REQ-API-017`；代理 CRUD、分享链接导入与检查复用协议引擎能力清单；桌面宿主通过 Tauri IPC 镜像命令语义。
- Verification commands: 尚未建立；实现后使用 OpenAPI 校验、API 集成测试、CSRF/限流测试和 WebSocket 重连测试。
- Rollout facts: 首个实现仅服务端提供 HTTP 控制面；桌面自动化 API 默认关闭。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 登录、会话、Token、CSRF、错误结构、分页、幂等、事件、健康检查、审计和代理 API 均未实现。
- 需要先冻结 OpenAPI 生成方式与错误码表，再建立契约测试。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
