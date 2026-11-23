# 浏览器运行时实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 状态机、进程监督、Xvfb 与沙箱契约已建立。

## Implementation Coverage

- Requirement coverage: 计划由 `mien-core` 的 runtime 模块、代理网关、服务端宿主和 Docker 入口脚本覆盖 `REQ-RUNTIME-001` 至 `REQ-RUNTIME-016`；协议引擎先于内核启动并在停止时关闭。
- Verification commands: 尚未建立；实现后使用生命周期集成测试、强杀对账测试、Docker 端到端测试和沙箱配置测试。
- Rollout facts: 首个实现仅覆盖 macOS Apple Silicon 与 Docker `linux/amd64`；Windows/Linux 桌面后续接入。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 进程启动、状态机、对账、代理引擎联动、CDP、Xvfb、openbox、noVNC、沙箱探测和日志轮转均未实现。
- 需要先验证目标容器环境的命名空间沙箱可行性，再确定默认容器安全配置。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
