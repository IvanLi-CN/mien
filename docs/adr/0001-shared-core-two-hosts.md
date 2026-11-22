# ADR-0001：共享 Rust 核心与双宿主

- 状态：已接受
- 决策范围：运行架构、桌面与服务端的代码复用边界

## 背景

产品必须同时支持 macOS Apple Silicon 桌面和 Docker 无桌面服务端。两种形态共享档案、指纹、代理、内核和运行时语义，但宿主能力不同：桌面需要系统钥匙串、窗口、自动更新和单实例；服务端需要 HTTP 认证、静态 Web UI、健康检查和容器信号处理。若为两种形态各写一套业务逻辑，指纹校验、启动参数和代理行为会很快分叉；若使用 Electron 单体，又会重复捆绑 Chromium，并增加与指纹内核并存的进程与升级复杂度。

## 决策

采用一个 Rust 核心库 `snailfish-core`，由两个宿主进程复用：

- `snailfish-desktop`：Tauri 2 宿主，负责窗口、WKWebView、Tauri IPC、系统钥匙串、自动更新、单实例和本地诊断入口。
- `snailfish-server`：axum 宿主，负责 HTTP/WebSocket、登录、Token、静态 Web UI、健康检查和优雅关闭。
- Web UI 使用同一套 React + TypeScript 构建产物；桌面通过 Tauri IPC 调用核心，服务端通过 HTTP API 调用核心。
- 核心库不依赖 Tauri 或 axum；平台差异通过密钥提供者、进程启动器、显示环境和文件权限 trait 注入。

## 备选方案

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| Electron 单体 | 前后端同栈，开发快 | 重复捆绑 Chromium；体积与内存高；进程模型与指纹内核冲突 | 不采用 |
| Tauri + 独立 sidecar 进程 | 桌面与服务端二进制完全一致 | sidecar 需单独签名、升级和 IPC；桌面认证与端口管理复杂 | 暂不采用 |
| 纯 Web + 远程服务 | 客户端最薄 | 桌面无法离线；本地文件与钥匙串集成弱；用户必须部署服务 | 不采用 |
| 共享核心 + 双宿主 | 业务单一实现；平台适配清晰；离线可用 | 需要维护两套薄宿主与两种传输 | 采用 |

## 后果

- 指纹、档案、代理和运行时行为只有一份实现，测试可以集中在核心库。
- 桌面与服务端的认证模型不同：桌面默认本地单用户，服务端有登录和 Token；核心只接受已认证的调用上下文。
- 需要维护 Tauri IPC 与 HTTP API 的薄适配层，避免把宿主类型泄漏到核心。
- 桌面自动化 API 默认关闭；需要时由桌面宿主启用回环 HTTP 服务并生成 Token。
- 服务端与桌面共用 Web UI，但服务端多出登录、会话和 Token 管理界面。

## 参考

- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- [`../specs/control-api/SPEC.md`](../specs/control-api/SPEC.md)
