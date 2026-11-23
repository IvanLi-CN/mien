# Mien 技术选型

## 1. 选型原则

- 本地优先：桌面端离线可用，不强制部署服务端。
- 单一核心：桌面与服务端共享 Rust 核心，业务规则只有一份实现。
- 少服务：v1 不引入 Postgres、Redis、消息队列或外部会话服务。
- 可验证：进程、网络、密钥和时间都通过可替换端口注入，便于测试。
- 安全边界清晰：内核、协议引擎、控制面、代理凭据和浏览器数据分层隔离。
- 许可可审计：链接依赖只接受宽松许可；协议引擎以独立进程分发并保留 MPL-2.0 许可与来源信息。
- 版本可回滚：内核、协议引擎、数据库和应用分别版本化，升级不静默改变已有档案。

## 2. 版本策略

- 版本表锁定大版本与最低小版本；具体补丁版本由 `Cargo.lock`、`bun.lock` 和构建清单固定。
- 内核与协议引擎使用完整版本号、平台和 SHA-256，不使用浮动标签。
- 升级分为应用升级、内核升级、协议引擎升级和数据库迁移；四者互不隐式联动。
- 安全补丁优先于功能升级；每次升级必须通过对应准入套件并保留回滚路径。

## 3. 技术栈总览

| 层 | 选型 | 版本策略 | 说明 |
| --- | --- | --- | --- |
| 目标平台 | macOS 13+ Apple Silicon；Docker `linux/amd64` | 固定 | macOS Intel、Linux ARM64 不在首个发布范围 |
| 核心语言 | Rust stable，edition 2024 | MSRV 1.85 | 内存安全、单二进制、可靠的进程与网络控制 |
| 异步运行时 | Tokio 1.x | 固定小版本 | 进程、网络、定时器和取消令牌 |
| 核心 HTTP | axum 0.8 | 固定小版本 | 基于 tower 生态，类型化提取器，易测试 |
| OpenAPI | utoipa | 固定小版本 | 从 Rust 类型生成契约，减少手写偏差 |
| 数据库 | SQLite 3.46+ | 固定最低版本 | 本地优先、无需额外服务、易备份 |
| 数据库访问 | sqlx 0.8 | 固定小版本 | 异步、编译期查询检查、内建迁移 |
| 序列化 | serde 1 + serde_json | 固定 | 领域 DTO、配置和事件 |
| 校验 | validator + jsonschema | 固定 | 类型字段校验与导入文件 schema 校验 |
| 错误 | thiserror；anyhow 仅宿主边界 | 固定 | 领域错误可分类，宿主错误可附加上下文 |
| 时间 | time 0.3 | 固定 | UTC 时间、序列化和测试时钟 |
| 标识 | ulid | 固定 | 档案、运行时和事件标识可排序 |
| 加密 | aes-gcm、argon2、sha2、zeroize、secrecy | 固定 | 字段级认证加密、口令派生、Token 哈希与内存清理 |
| 密钥 | keyring 3 | 固定 | macOS 钥匙串；服务端文件/环境变量适配 |
| HTTP 客户端 | reqwest + rustls | 固定 | 代理检查回显、更新检查；不依赖系统 OpenSSL |
| 进程 | tokio::process；Unix 用 nix | 固定 | 参数数组、进程组、信号和超时终止 |
| 系统信息 | sysinfo | 固定 | 内存、磁盘和进程准入检查 |
| 文件锁 | fd-lock | 固定 | 档案锁与单实例；避免陈旧锁文件 |
| CLI | clap 4 | 固定 | 迁移、诊断和准入辅助 |
| 日志 | tracing + tracing-subscriber | 固定 | 结构化 JSON、字段注入、脱敏 |
| 桌面 | Tauri 2 + WKWebView | 固定主版本 | 体积小、原生进程控制、macOS 集成 |
| 桌面 IPC | Tauri commands + `@tauri-apps/api`（`invoke`/`listen`）+ ts-rs | 固定主版本 | 进程内调用、无本地 HTTP 管理端口、DTO 生成 TypeScript 类型 |
| 前端传输 | `ControlTransport` 抽象 + Tauri/HTTP 适配器 | 内部接口 | 同一套领域服务适配桌面与服务端 |
| 文件对话框 | tauri-plugin-dialog | 固定主版本 | 导入导出路径由用户选择并再次校验 |
| 外链 | tauri-plugin-opener | 固定主版本 | 外部链接交给系统浏览器，不在 WebView 导航 |
| 桌面安全 | Tauri capabilities + CSP | 固定配置 | 只授权主窗口与必要命令，禁止远程内容 |
| Web UI | React 19 + TypeScript 5 | 固定主版本 | 桌面与服务端共用，生态成熟 |
| 构建 | Vite 7 | 固定主版本 | 快速开发与静态构建 |
| 服务端状态 | TanStack Query 5 | 固定主版本 | 缓存、重试、请求失效 |
| 路由 | React Router 7 | 固定主版本 | 嵌套路由与数据加载 |
| 样式 | Tailwind CSS 4 | 固定主版本 | 设计令牌与响应式布局 |
| 组件原语 | Radix UI | 固定主版本 | 可访问性与键盘交互 |
| 表单 | React Hook Form + Zod | 固定主版本 | 表单状态与客户端校验 |
| 国际化 | i18next + react-i18next | 固定主版本 | 中英文与命名空间 |
| API 客户端 | openapi-typescript + openapi-fetch | 固定主版本 | 从 OpenAPI 生成类型安全客户端 |
| 前端质量 | Biome 2 + Vitest + Testing Library | 固定主版本 | 格式化、lint、单元与组件测试 |
| 端到端 | Playwright | 固定主版本 | CDP 连接、桌面与服务端流程 |
| 包管理 | Bun 1.x；Node 22 LTS 用于工具链 | 固定主版本 | 安装快；Playwright/Tauri 兼容 Node |
| 浏览器内核 | fingerprint-chromium 148.0.7778.215 | 固定 | 上游 macOS 构建与 canvas 稳定性最优 |
| 协议引擎 | Xray-core v26.11.9+ | 固定补丁版本与 SHA-256 | 单实例动态入站/出站/路由；VLESS/VMess/SS/Trojan/Hysteria2/WireGuard；MPL-2.0 |
| 容器基础 | Debian bookworm-slim | 固定摘要 | 需要 X11、Chromium 运行库与字体 |
| 容器 init | tini | 固定 | 信号转发与僵尸进程回收 |
| 虚拟显示 | Xvfb + openbox | 固定主版本 | 无桌面环境的有头运行 |
| 远程查看 | x11vnc + noVNC/websockify | 固定主版本 | 可选、一次性令牌、默认关闭 |
| 任务编排 | cargo xtask | 固定 | 仓库内构建、校验、打包任务 |
| CI | GitHub Actions macOS ARM64 + linux/amd64 | 固定 | 原生平台构建、签名、镜像与扫描 |
| 制品安全 | cargo-deny、cargo-audit、Trivy、Syft、cosign | 固定主版本 | 许可、漏洞、SBOM 与签名 |

## 4. 关键选型对比

### 4.1 Rust 核心 vs Go / Node

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| Rust 核心 | 内存安全、强类型、单二进制、与 Tauri 同栈、进程控制可靠 | 开发速度较慢、异步生命周期复杂 | 采用 |
| Go 核心 | 进程与网络实现简单、部署方便 | 与 Tauri/桌面集成弱、需要两套工具链、类型表达较弱 | 不采用 |
| Node/TypeScript 核心 | 前后端同栈 | 进程监督、密钥与系统集成弱；运行时依赖大 | 不采用 |

### 4.2 Tauri 2 vs Electron

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| Tauri 2 | 体积小、系统 WebView、原生命令与钥匙串、无需再捆绑 Chromium | WKWebView 与 Chromium 存在渲染差异，需要跨浏览器测试 | 采用 |
| Electron | 渲染一致、生态成熟 | 重复捆绑 Chromium、体积与内存高、与指纹内核进程模型冲突 | 不采用 |

### 4.3 SQLite vs Postgres

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| SQLite | 零运维、单文件、易备份、适合单机与单容器 | 单写者、并发写入受限 | 采用 |
| Postgres | 并发与多租户能力强 | 需要额外服务、备份和升级；v1 不需要 | 后续评估 |
| 嵌入式 KV | 简单 | 事务、查询和审计能力不足 | 不采用 |

### 4.4 axum + sqlx vs Actix + Diesel/SeaORM

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| axum + sqlx | tower 生态、类型化提取、异步 SQL、迁移内建 | 需要手写部分 SQL 与映射 | 采用 |
| Actix + Diesel | 成熟、同步 ORM 强 | 运行时与异步生态分裂；迁移与测试成本高 | 不采用 |
| axum + SeaORM | ORM 抽象高 | 复杂查询与性能调优可控性较差 | 不采用 |

### 4.5 Xray-core vs sing-box vs 自研协议

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| sing-box | 协议覆盖广、单二进制、跨平台 | 无动态入站/出站/路由 CRUD；SIGHUP 全量重建；多实例连接池违反节点连接约束；GPL-3.0 | 不采用 |
| Xray-core | MPL-2.0、动态 HandlerService/RoutingService、Hysteria2、共享出站连接复用 | 共享故障域；API 是高权限控制面；不支持 TUIC | 采用 |
| 自研协议 | 单进程、完全可控 | 密码学与传输维护成本极高 | 不采用 |

### 4.6 REST + WebSocket vs gRPC / GraphQL

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| REST + WebSocket | 浏览器友好、调试简单、OpenAPI 成熟 | 事件需自定义回放 | 采用 |
| gRPC | 强契约、流式 | 浏览器与 Tauri 客户端复杂度高 | 不采用 |
| GraphQL | 灵活查询 | 授权、缓存和复杂度控制成本高 | 不采用 |

### 4.7 React + Vite vs Svelte / Vue

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| React 19 + Vite | 生态、组件库、测试工具最成熟 | 包体与状态管理需要约束 | 采用 |
| Svelte | 包体小、语法简洁 | 生态与团队熟悉度较低 | 不采用 |
| Vue | 上手快、生态成熟 | 与现有 React 组件资产不匹配 | 不采用 |

### 4.8 Tailwind + Radix vs 成品组件库

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| Tailwind + Radix | 设计可控、可访问性原语、无重主题负担 | 需要自建组件层 | 采用 |
| MUI / Ant Design | 开箱即用 | 视觉同质化、主题覆盖成本高、包体大 | 不采用 |

### 4.9 Xvfb 有头 vs 纯无头

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| Xvfb + openbox | 有头行为、指纹一致性更好 | 镜像与内存开销 | 默认 |
| 纯无头 | 资源低 | 指纹保证明显下降 | 显式降级 |

### 4.10 自建会话 vs 外部会话服务

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| SQLite 会话表 | 无额外服务、可审计、易备份 | 仅适合单实例 | 采用 |
| Redis 会话 | 多实例共享 | 引入外部依赖与运维 | v1 不需要 |

### 4.11 Tauri IPC vs 本地 HTTP 管理面

| 方案 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- |
| Tauri IPC | 无端口、无 Token 注入、进程内调用、攻击面小 | 需要 IPC 适配层、事件回放和命令契约测试 | 采用 |
| 本地 axum HTTP | 桌面与服务端传输完全一致 | 端口、Token、CORS、CSRF、监听面与竞态 | 不采用 |
| Electron + Node 后端 | 渲染一致 | 重复捆绑 Chromium、体积与内存高 | 不采用 |

## 5. 依赖与许可策略

- Rust 链接依赖允许 MIT、Apache-2.0、BSD、ISC、Zlib、Unicode；拒绝 GPL/AGPL/SSPL 进入链接依赖。
- `cargo-deny` 检查许可证、重复版本、禁用源与安全公告；`cargo-audit` 作为补充。
- 前端依赖允许 MIT、Apache-2.0、BSD、ISC；锁文件提交并定期审计。
- fingerprint-chromium 为 BSD-3-Clause；发布物保留版权与许可声明。
- Xray-core 为 MPL-2.0，作为独立进程分发，不链接；发布物保留许可证、来源信息并完成再分发合规评审。
- TUIC 不进入 v1；协议矩阵与 Xray 能力清单、测试结果绑定。
- 所有发布物生成 SBOM；容器镜像固定摘要并签名。

## 6. 构建与发布

- Rust workspace：`mien-core`、`mien-server`、`mien-desktop`、`mien-cli`、`xtask`。
- 前端：`ui/`，由 Bun 安装依赖，Vite 构建静态资源。
- 本地检查：`cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo nextest run`、`biome check`、`tsc --noEmit`、`vitest run`。
- 合约检查：OpenAPI 生成差异、JSON Schema 编译、事件 schema、导入导出 fixture。
- 端到端：Playwright 连接 CDP；Docker 有头/无头；受控协议节点；DNS canary。
- macOS：`aarch64-apple-darwin` 构建，嵌套内核与 Xray-core 签名，公证与 stapler 验证。
- Docker：`buildx` 构建 `linux/amd64`，非 root、只读根、`/data` 卷、健康检查、SBOM、Trivy、cosign。
- 内核与引擎：构建时下载、SHA-256 校验、写入能力清单；不提交二进制。

## 7. 测试工具链

| 层 | 工具 | 说明 |
| --- | --- | --- |
| Rust 单元/集成 | cargo-nextest | 并行、超时与重试控制 |
| 属性测试 | proptest | 种子、路径、配置组合 |
| 前端单元 | Vitest + Testing Library | 组件与 hooks |
| 端到端 | Playwright | CDP、桌面与服务端流程 |
| 契约 | utoipa、ajv、JSON Schema | OpenAPI 与 schema 校验 |
| 协议节点 | Xray-core 测试镜像 | VLESS、VMess、SS、Trojan、Hysteria2 |
| 漏洞 | Trivy、cargo-audit、依赖审计 | 镜像与依赖 |
| 许可 | cargo-deny、SBOM | 链接依赖与发布物 |

## 8. 主要风险

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| WKWebView 与 Chromium 渲染差异 | UI 在桌面与服务端表现不同 | Playwright 覆盖关键页面；CSS 约束；视觉证据 |
| SQLite 单写者 | 并发写操作排队或超时 | 短事务、WAL、单写者队列、错误重试 |
| Xray MPL-2.0 与共享引擎合规 | 再分发与共享故障域 | 独立进程、许可证与来源信息、再分发评审、失败关闭与重建 |
| 共享引擎崩溃或 API 不可用 | 所有运行档案网络中断 | 失败关闭、全局事件、有界重启、原端口重建、不直连回退 |
| Docker 沙箱不可用 | 安全边界削弱 | 启动前检测、默认拒绝、显式无沙箱开关与审计 |
| Apple Silicon 模拟 amd64 | 性能下降 | 文档标注；性能基线在 x86_64 测量；原生 ARM64 后续 |
| 上游内核缺陷 | 指纹或稳定性问题 | 固定 148、准入测试、沙箱、回滚 |
| 依赖升级破坏兼容 | 构建或运行时故障 | 锁文件、MSRV、cargo-deny、分阶段升级 |

## 9. 版本锁定摘要

| 组件 | 首个发布基线 |
| --- | --- |
| fingerprint-chromium | `148.0.7778.215` |
| Xray-core | `v26.11.9` 或经准入的后继补丁版本，构建时锁定 SHA-256 |
| Rust | stable，edition 2024，MSRV 1.85 |
| Tauri | 2.x |
| React / TypeScript / Vite | 19.x / 5.x / 7.x |
| axum / sqlx / Tokio | 0.8.x / 0.8.x / 1.x |
| Node / Bun | 22 LTS / 1.x |
| Debian | bookworm-slim，固定镜像摘要 |

## 10. 参考

- [`ARCHITECTURE.md`](ARCHITECTURE.md)
- [`DESIGN.md`](DESIGN.md)
- [`PRD.md`](PRD.md)
- [`SECURITY.md`](SECURITY.md)
- [`TEST-PLAN.md`](TEST-PLAN.md)
- [`adr/`](adr/)
- [`specs/`](specs/)
