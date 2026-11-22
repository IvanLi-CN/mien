# Snailfish

Snailfish 是一个基于 [fingerprint-chromium](https://github.com/adryfish/fingerprint-chromium) 的多环境指纹浏览器产品，提供 macOS 桌面应用与 Docker 服务端两种运行形态，面向需要稳定隔离、可复现指纹和自动化控制的多账号运营、测试与隐私研究场景。

## 运行形态

| 形态 | 目标环境 | 用户界面 | 浏览器运行模式 | 首个发布 |
| --- | --- | --- | --- | --- |
| macOS 桌面 | Apple Silicon，macOS 13+ | Tauri 2 原生窗口承载同一套 Web UI | 有头（宿主桌面）；无头为显式降级选项 | 支持 |
| Docker 服务 | `linux/amd64` 容器，无桌面环境 | 浏览器访问容器内服务提供的 Web UI | 有头（Xvfb + openbox，可选 noVNC 查看）；无头为显式降级选项 | 支持 |
| Windows 桌面 | Windows 10/11 x64 | 同一套 Web UI | 有头；无头可选 | 后续 |
| Linux 桌面 | Linux x64 | 同一套 Web UI | 有头；无头可选 | 后续 |

Apple Silicon 上的 Docker Desktop 或 OrbStack 以 `linux/amd64` 模拟运行服务镜像；原生 `linux/arm64` 需要自行从源码构建指纹内核，不在首个发布范围内。

## 浏览器内核基线

首个发布固定使用 fingerprint-chromium `148.0.7778.215`：macOS ARM64 DMG、Linux x86_64 TAR.XZ、Windows x64 ZIP 均由上游提供。选择该版本而不是更新的 `150.0.7871.186`，原因包括上游尚无可用的 macOS 构建，以及 150 在启用 canvas 伪装时存在已复现的渲染进程崩溃与站点加载失败；148 同时具备已公开源码，便于安全审计。

内核不修改、不分叉。产品通过命令行开关、能力清单、校验和与版本绑定来使用上游二进制，升级必须经过独立内核准入测试。

## 协议引擎基线

正向代理由每档案独立的 [sing-box](https://github.com/SagerNet/sing-box) 协议引擎承载，首个发布覆盖 VLESS、VMess、Shadowsocks、Trojan，并按能力清单支持 Hysteria2、TUIC、WireGuard。HTTP/HTTPS/SOCKS5 也走同一引擎，Chromium 只连接回环 mixed 入口，代理凭据不进入浏览器进程。sing-box 以 GPL-3.0-or-later 分发，发布物必须保留许可证、源码链接或书面要约，并通过法律评审。

## 当前状态

仓库处于设计与契约阶段，尚未包含实现代码。产品上下文、视觉系统 seed 与 Profiles surface brief 已锁定；本目录中的文档覆盖产品需求、系统架构、详细设计、技术选型、决策记录、主题规格、路线图、测试计划与安全威胁模型。

## 文档地图

| 文档 | 内容 |
| --- | --- |
| [`PRODUCT.md`](PRODUCT.md) | 产品事实：用户、目的、定位、约束、证据与产品原则 |
| [`DESIGN.md`](DESIGN.md) | 已锁定的视觉系统 seed：Press Proof 世界、色彩、字体、布局与规则 |
| [`.impeccable/surfaces/ui-src-pages-profiles.md`](.impeccable/surfaces/ui-src-pages-profiles.md) | Profiles 管理面的 surface brief 与方向契约 |
| [`docs/PRD.md`](docs/PRD.md) | 产品定位、用户、功能与非功能需求、范围、验收与风险 |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | 系统边界、组件、部署拓扑、数据模型、关键流程与失败处理 |
| [`docs/DESIGN.md`](docs/DESIGN.md) | 工程详细设计：模块接口、启动/停止事务、并发、错误、配置、存储与部署契约 |
| [`docs/TECH-STACK.md`](docs/TECH-STACK.md) | 技术选型、替代方案、版本策略、许可与构建发布 |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | 里程碑、交付物与退出标准 |
| [`docs/TEST-PLAN.md`](docs/TEST-PLAN.md) | 指纹、代理、生命周期、安全与发布验证矩阵 |
| [`docs/SECURITY.md`](docs/SECURITY.md) | 资产、威胁模型、控制措施与残余风险 |
| [`docs/GLOSSARY.md`](docs/GLOSSARY.md) | 领域术语与缩写 |
| [`docs/adr/`](docs/adr/) | 架构与边界决策记录 |
| [`docs/specs/`](docs/specs/) | 长期能力契约，按主题维护 |
| [`docs/contracts/`](docs/contracts/) | 指纹规格、内核能力清单与档案导出的 JSON Schema |

## 主题规格

- [`docs/specs/profile-management/SPEC.md`](docs/specs/profile-management/SPEC.md)：环境档案、分组、导入导出与生命周期元数据。
- [`docs/specs/fingerprint-engine/SPEC.md`](docs/specs/fingerprint-engine/SPEC.md)：指纹规格、种子、一致性校验与启动参数生成。
- [`docs/specs/browser-runtime/SPEC.md`](docs/specs/browser-runtime/SPEC.md)：内核进程、有头/无头模式、监督、崩溃恢复与显示环境。
- [`docs/specs/proxy-gateway/SPEC.md`](docs/specs/proxy-gateway/SPEC.md)：VLESS/VMess/Shadowsocks/Trojan/Hysteria2、协议引擎、分享链接、远程 DNS 与泄漏防护。
- [`docs/specs/control-api/SPEC.md`](docs/specs/control-api/SPEC.md)：控制面 HTTP/WS 契约、鉴权、事件与自动化入口。

## 关键产品约束

- 指纹内核不能凭空创造宿主不存在的显示属性。`screen.width`、`screen.height`、`devicePixelRatio` 与字体枚举仍受宿主显示器和已安装字体影响；跨操作系统画像只能做到尽力对齐，不能承诺与真实设备完全一致。
- 无头模式不是有头模式的等价替代。fingerprint-chromium 只处理了部分 Headless 特征，其余特征仍可能暴露；无头档案会被标记为“降低保证”。
- Chromium 不支持代理认证和 VLESS/VMess/Shadowsocks 等协议。产品为每个运行档案启动独立的 sing-box 协议引擎，内核只看到回环 mixed 入口，凭据不进入浏览器进程。
- sing-box 以 GPL-3.0-or-later 分发。产品将其作为独立进程使用，发布物保留许可、源码链接或书面要约，并在发布前完成法律评审。
- 指纹浏览器提供环境隔离与指纹一致性，不提供匿名性保证，也不承诺绕过任何平台的风控或服务条款。
- 上游项目明确不提供技术支持，源码按延迟策略发布。内核缺陷只能通过版本准入、沙箱、功能降级或自建补丁分支应对。

## 开发入口

实现开始前应先确认 [`docs/PRD.md`](docs/PRD.md) 的范围与验收，再阅读 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)、[`docs/DESIGN.md`](docs/DESIGN.md) 与 [`docs/TECH-STACK.md`](docs/TECH-STACK.md) 建立工作区，并以 [`docs/specs/`](docs/specs/) 中的 `REQ-*` 与 `VER-*` 作为实现和验证的契约。
