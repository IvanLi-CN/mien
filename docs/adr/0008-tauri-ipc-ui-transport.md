# ADR-0008：管理网页通过 Tauri IPC 调用核心

- 状态：已接受
- 决策范围：桌面 Web UI 与 Rust 核心的调用方式、事件订阅、DTO 契约与自动化 API 边界

## 背景

桌面端与服务端共用一套 Web UI。服务端通过 HTTP 与 WebSocket 调用控制面；桌面端运行在 Tauri 2 的 WKWebView 中，业务逻辑在同一个进程的 Rust 核心。若桌面 UI 也通过 `http://127.0.0.1:<port>` 调用本地 axum 服务，会额外引入监听端口、Token 注入、CORS、CSRF、端口冲突和本地攻击面；若为桌面另写一套 UI，又会失去共享代码和一致交互。需要一种不开放本地 HTTP 控制面、同时保持服务端 HTTP 契约的技术边界。

## 决策

- 管理网页依赖一个 `ControlTransport` 抽象，领域服务只调用 `profiles.list`、`runtime.start` 等业务方法，不直接使用 `fetch` 或 `invoke`。
- 桌面实现使用 Tauri IPC：
  - 前端通过 `@tauri-apps/api/core` 的 `invoke` 调用 Rust 命令。
  - Rust 通过 `#[tauri::command]` 暴露薄适配函数，直接调用 `snailfish-core` 用例。
  - 事件通过 `@tauri-apps/api/event` 的 `listen` 订阅统一频道 `snailfish://event`；Rust 使用 Tauri `Emitter` 发送同一个 `DomainEvent` DTO。
- 服务端实现使用 HTTP/WebSocket：同一套领域服务调用 `fetch('/api/v1/...')` 和 `/api/v1/events`。
- 桌面 UI 不启动本地 HTTP 控制面。桌面自动化 API 是独立可选能力：显式开启、只绑定回环、使用 Bearer Token，与 UI 的 IPC 通道分离。
- DTO 定义在共享 API 类型层（Rust `serde` + `ts-rs` 生成 TypeScript）；HTTP 侧用 `utoipa` 生成 OpenAPI。命令参数名通过 `#[tauri::command(rename_all = ...)]` 明确固定，避免前端与 Rust 命名漂移。
- 命令命名使用 `<domain>_<action>`，例如 `profiles_list`、`profiles_create`、`runtime_start`、`proxies_import_link`、`events_after`。
- 错误统一为 `ApiError { code, message, request_id, retryable, details }`；Tauri 命令返回 `Result<T, ApiError>`，HTTP 侧映射到相同结构与状态码。
- 事件先写入 `event_log`，再通过 Tauri 事件频道广播。WebView 刷新或重载后，UI 先调用 `events_after(last_event_id)` 补齐，再根据事件标识去重。
- 文件导入导出使用 Tauri dialog 插件选择路径，命令只接受用户选定的路径并再次校验数据根、大小和 schema；WebView 不能直接读取任意本地路径。
- Tauri 2 capabilities 只授权主窗口调用所需命令；生产 CSP 禁止远程脚本与内联脚本；外部链接交给系统浏览器打开，不在 WebView 内导航到远程页面。

## 备选方案

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| 桌面 UI 通过本地 axum HTTP | 桌面与服务端传输完全一致 | 端口、Token、CORS、CSRF、监听面与竞态；UI 加载前需注入凭据 | 不采用 |
| 桌面 UI 直接调用 Tauri 命令 | 无端口、无额外认证、进程内调用、类型可控 | 需要维护 IPC 适配层和事件回放 | 采用 |
| 桌面另写原生 UI | 平台体验最好 | 两套 UI、两套交互与测试 | 不采用 |
| Electron + Node 后端 | 渲染一致、生态成熟 | 重复捆绑 Chromium、体积与内存高 | 不采用 |
| Tauri 插件 HTTP 从 WebView 直连核心 | 复用 HTTP 习惯 | 仍需监听端口与认证；插件边界不清 | 不采用 |

## 后果

- UI 领域服务与传输解耦；桌面和服务器共用页面、表单、错误展示和事件处理。
- 桌面端没有为管理 UI 开放的本地 HTTP 端口，减少本地攻击面；自动化 API 仍是独立、默认关闭的入口。
- 需要维护 Tauri 命令注册表、DTO 绑定和 IPC 契约测试；命令是应用内部接口，不承诺跨应用版本兼容。
- 事件订阅需要“先订阅、再补齐、按标识去重”的客户端逻辑；`event_log` 的保留策略决定可回放窗口。
- 文件导入导出必须经过 dialog 与路径校验；不能把 WebView 传来的任意路径直接交给核心。
- Tauri capabilities、CSP 和导航限制成为安全验收项；远程内容不得在主 WebView 中加载。

## 参考

- [`ADR-0001`](0001-shared-core-two-hosts.md)
- [`../DESIGN.md`](../DESIGN.md)
- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- [`../specs/control-api/SPEC.md`](../specs/control-api/SPEC.md)
