# 浏览器运行时主题历史

> 本文件记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- 主题在开发启动前建立，运行模式覆盖 macOS 宿主桌面与 Docker 无桌面环境。
- 有头模式是默认；无头模式是显式降级选项，状态与 API 必须保留该语义。
- 共享 Xray 引擎先于内核就绪；启动时先为该运行档案创建回环 HTTP 入站和路由，停止时在内核退出后删除入站和路由，但共享引擎继续为其他档案运行。
- 共享引擎故障会影响所有运行档案；失败关闭、重启和按原端口重建由代理网关负责。
- 容器重启和控制面崩溃属于常规恢复路径，不作为异常终止处理。

## Replacements / Background

- 选择共享 Xvfb 而不是每档案独立虚拟显示，以控制内存和进程开销；代价是屏幕属性在档案间共享。
- 选择直接执行内核二进制而不是 `open -a`，避免 macOS 丢失启动参数。
- 选择由产品校验内核校验和与沙箱状态，因为上游不提供这些运行时保证。
- noVNC 被设计为可选高权限入口，默认关闭，避免把容器变成可远程控制的桌面。

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`../../adr/0005-headful-first-display.md`](../../adr/0005-headful-first-display.md)
