# 术语表

本表固定产品、架构与测试文档中的领域词义。实现中的类型名、字段名与 API 名称保持英文，文档叙述使用中文。

| 术语 | 英文 / 标识 | 含义 |
| --- | --- | --- |
| 环境档案 | profile | 一个可独立启动、拥有独立浏览器数据目录、代理和指纹规格的账号环境。 |
| 档案标识 | profile ID | 不可变的 ULID，用于 API、目录和日志关联；名称可重复，标识不重复。 |
| 指纹规格 | fingerprint spec | 描述一个档案期望呈现的浏览器环境的结构化配置，包含种子、平台、品牌、语言、时区、硬件与伪装开关。 |
| 种子 | fingerprint seed | 传给 `--fingerprint` 的 32 位无符号整数；内核据此派生 canvas、音频、WebGL、设备内存等未显式暴露的指纹面。 |
| 派生特征 | derived trait | 不由用户逐项设置、而由种子和内核版本决定的指纹属性，例如 canvas 噪声、音频噪声、GPU 参数集。 |
| 指纹内核 | kernel | fingerprint-chromium 的可执行发行包，包含 Chromium 与其指纹补丁。 |
| 能力清单 | capability manifest | 某个内核或协议引擎版本与平台组合实际支持的开关、协议、取值和已知缺陷清单；启动前据此校验档案。 |
| 运行时 | runtime | 由产品监督的一个内核进程实例及其运行档案回环 HTTP 入站、路由、调试端口、日志和显示环境；共享协议引擎不属于单个运行时。 |
| 启动参数 | launch argv | 由指纹引擎生成、以参数数组形式传给内核的完整命令行；不经过 shell。 |
| 有头模式 | headful | 内核创建可见窗口并使用图形显示服务的运行方式。macOS 使用宿主桌面；Docker 使用 Xvfb 虚拟显示。 |
| 无头模式 | headless | 内核不创建可见窗口的运行方式。指纹保证弱于有头模式，属于显式降级选项。 |
| 虚拟显示 | Xvfb | Docker 中提供无物理显示器图形环境的 X 服务器。 |
| 窗口管理器 | openbox | Docker 有头模式下的轻量窗口管理器，用于让 Chromium 窗口获得正常的焦点、尺寸和状态行为。 |
| 远程查看 | noVNC | 通过 WebSocket 在浏览器中查看 Xvfb 画面的可选组件；不是运行内核的必需依赖。 |
| 代理网关 | proxy gateway | 核心库中的监督组件，负责代理配置校验、Xray 管理 API 调用、共享出站引用计数、入站/路由生命周期、就绪探测和失败事件；不转发字节流。 |
| 协议引擎 | protocol engine | 产品固定并校验的 Xray-core 可执行文件，每宿主一个共享进程，负责 VLESS、VMess、Shadowsocks、Trojan、Hysteria2 等协议、认证、传输与出站 DNS。 |
| 运行档案入站 | runtime inbound | Xray 在回环地址上为单个运行档案创建的 HTTP 入口；Chromium 只连接该入口。 |
| 共享出站 | shared outbound | 由节点、凭据、传输、TLS/REALITY 与 mux 设置确定的 Xray 出站；相同 key 的运行档案共享其连接池。 |
| 正向代理 | forward proxy | 客户端侧代理，浏览器把请求交给本地代理，再由代理节点访问目标站点。 |
| 代理节点 | proxy node | 用户配置的 HTTP、HTTPS、SOCKS5、VLESS、VMess、Shadowsocks、Trojan、Hysteria2 等远端服务。 |
| 分享链接 | share link | `vless://`、`vmess://`、`ss://`、`trojan://`、`hysteria2://` 形式的单节点配置文本。 |
| 远程 DNS | remote DNS | 目标域名由代理节点解析或随协议发送，避免本机 DNS 查询泄漏目标域名；节点主机名可本地 bootstrap 解析。 |
| 失败关闭 | fail-closed | 协议引擎不可用或出站失败时浏览器直接失去网络，而不是回退到直连或其他代理。 |
| 控制面 | control plane | 产品自身的 HTTP/WebSocket API 与 Web UI，用于管理档案和运行时。 |
| 服务端宿主 | server host | 运行控制面、内核、共享协议引擎与显示环境的 Docker 容器进程。 |
| 桌面宿主 | desktop host | macOS 上的 Tauri 应用进程，承载 Web UI 并调用共享核心库。 |
| Tauri IPC | Tauri IPC | 桌面 WebView 与 Rust 宿主之间的进程内调用通道；前端用 `invoke`，Rust 用 `#[tauri::command]`。 |
| 控制传输 | ControlTransport | Web UI 的传输抽象；桌面实现调用 Tauri IPC，服务端实现调用 HTTP/WebSocket。 |
| 自动化 API | automation API | 面向脚本的外部控制入口；桌面默认关闭，启用后只绑定回环并使用 Bearer Token。 |
| 指纹探针 | fingerprint probe | 在页面内采集 UA、Client Hints、时区、语言、WebGL、canvas、音频、字体等属性的测试页或脚本。 |
| 主机对齐 | host-aligned | 档案声明的操作系统与宿主操作系统一致，从而减少字体、显示和图形栈泄漏。 |
| 跨系统画像 | cross-OS persona | 档案声明的操作系统与宿主不同，例如在 Linux 容器中声明 Windows。属于高级用法，保证较弱。 |
| 降低保证 | reduced assurance | 产品明确标记某运行模式或配置无法达到基线指纹一致性，例如无头模式或跨系统画像。 |
| 主密钥 | master key | 用于加密数据库中代理凭据、UUID、密码、私钥等敏感字段的 32 字节密钥；桌面存入系统钥匙串，服务端存入受权限保护的文件或环境变量。 |
| 信封加密 | envelope encryption | 使用主密钥加密数据密钥或字段，密文与元数据分离存储的方式。 |
| CDP | Chrome DevTools Protocol | Chromium 的调试协议，用于自动化控制和指纹探针采集。 |
| UA-CH | User-Agent Client Hints | 浏览器通过 `navigator.userAgentData` 与请求头暴露的结构化客户端信息。 |
| 沙箱 | Chromium sandbox | Chromium 的渲染进程隔离机制；容器内是否启用是安全验收项。 |
| 内核准入 | kernel qualification | 新内核版本进入产品前必须通过的指纹、稳定性、安全与兼容测试。 |
| VLESS | VLESS | 基于用户 ID 的轻量代理协议，常与 TLS、REALITY、WebSocket 或 gRPC 组合。 |
| VMess | VMess | 基于用户 ID 与加密选型的代理协议，支持多种传输。 |
| Shadowsocks | Shadowsocks | 基于 AEAD 或 2022 方法的加密代理协议。 |
| Trojan | Trojan | 以 TLS 伪装为基础的密码认证代理协议。 |
| Hysteria2 | Hysteria2 | 基于 QUIC 的代理协议，支持混淆与拥塞控制。 |
