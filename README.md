# Mien

Mien 是一个基于 [fingerprint-chromium](https://github.com/adryfish/fingerprint-chromium) 的多环境指纹浏览器产品，提供 macOS 桌面应用与 Docker 服务端两种运行形态，面向需要稳定隔离、可复现指纹和自动化控制的多账号运营、测试与隐私研究场景。

## 运行形态

| 形态 | 目标环境 | 用户界面 | 浏览器运行模式 | 首个发布 |
| --- | --- | --- | --- | --- |
| macOS 桌面 | Apple Silicon，macOS 13+ | Tauri 2 原生窗口承载同一套 Web UI | 有头（宿主桌面）；无头/伪有头为显式降级选项 | 支持 |
| Docker 服务 | `linux/amd64` 容器，无桌面环境 | 浏览器访问容器内服务提供的 Web UI | 有头（Xvfb + openbox，可选 noVNC 查看）；无头/伪有头为显式降级选项 | 支持 |
| Windows 桌面 | Windows 10/11 x64 | 同一套 Web UI | 有头；无头/伪有头可选 | 后续 |
| Linux 桌面 | Linux x64 | 同一套 Web UI | 有头；无头/伪有头可选 | 后续 |

Apple Silicon 上的 Docker Desktop 或 OrbStack 以 `linux/amd64` 模拟运行服务镜像；原生 `linux/arm64` 需要自行从源码构建指纹内核，不在首个发布范围内。

macOS 应用使用 AdHoc 签名、不公证；首次启动会被 Gatekeeper 阻止。用户应先校验 SHA-256 或 artifact attestation，再按 [`docs/RELEASE.md`](docs/RELEASE.md) 手动放行。

## 浏览器内核基线

首个发布固定使用 fingerprint-chromium `148.0.7778.215`：macOS ARM64 DMG、Linux x86_64 TAR.XZ、Windows x64 ZIP 均由上游提供。选择该版本而不是更新的 `150.0.7871.186`，原因包括上游尚无可用的 macOS 构建，以及 150 在启用 canvas 伪装时存在已复现的渲染进程崩溃与站点加载失败；148 同时具备已公开源码，便于安全审计。

首个发布不修改、不分叉内核。产品通过命令行开关、能力清单、校验和与版本绑定来使用上游二进制，升级必须经过独立内核准入测试；补丁分支只作为后续安全兜底，不属于首个发布。

## 协议引擎基线

正向代理由每宿主一个共享的 [Xray-core](https://github.com/XTLS/Xray-core) 协议引擎承载，首个发布覆盖 HTTP、HTTPS、SOCKS5、VLESS、VMess、Shadowsocks、Trojan，并按能力清单支持 Hysteria2、WireGuard；TUIC 不进入 v1。每个运行档案有独立回环 HTTP 入站和路由，相同节点/凭据共享出站与连接池；Chromium 不接触代理凭据。Xray-core 以 MPL-2.0 分发，发布物必须保留许可证与来源信息。

## 当前状态

仓库处于设计与契约阶段，尚未包含实现代码。产品上下文、Deep Current 视觉系统与 Profiles 首屏 comp 已锁定；可执行 API/事件/错误/数据库/Xray 映射契约、CI/发布设计与 M0 计划已补齐。

## 文档地图

| 文档 | 内容 |
| --- | --- |
| [`PRODUCT.md`](PRODUCT.md) | 产品事实：用户、目的、定位、约束、证据与产品原则 |
| [`DESIGN.md`](DESIGN.md) | 已锁定的 Deep Current 视觉系统：深色空间、身份色、布局、动效与可访问性规则 |
| [`docs/design/profiles-dark.png`](docs/design/profiles-dark.png) | 已批准的 Profiles 首屏视觉 comp，1440×900 |
| [`.impeccable/surfaces/ui-src-pages-profiles.md`](.impeccable/surfaces/ui-src-pages-profiles.md) | Profiles 管理面的 surface brief 与方向契约 |
| [`docs/PRD.md`](docs/PRD.md) | 产品定位、用户、功能与非功能需求、范围、验收与风险 |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | 系统边界、组件、部署拓扑、数据模型、关键流程与失败处理 |
| [`docs/DESIGN.md`](docs/DESIGN.md) | 工程详细设计：模块接口、启动/停止事务、并发、错误、配置、存储与部署契约 |
| [`docs/TECH-STACK.md`](docs/TECH-STACK.md) | 技术选型、替代方案、版本策略、许可与构建发布 |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | 里程碑、交付物与退出标准 |
| [`docs/TEST-PLAN.md`](docs/TEST-PLAN.md) | 指纹、代理、生命周期、安全、UI 与发布验证矩阵 |
| [`docs/SECURITY.md`](docs/SECURITY.md) | 资产、威胁模型、控制措施与残余风险 |
| [`docs/GLOSSARY.md`](docs/GLOSSARY.md) | 领域术语与缩写 |
| [`docs/adr/`](docs/adr/) | 架构与边界决策记录 |
| [`docs/specs/`](docs/specs/) | 长期能力契约，按主题维护 |
| [`docs/contracts/`](docs/contracts/) | 长期契约的 JSON Schema、OpenAPI、事件 schema 与内核/引擎清单 |
| [`docs/contracts/openapi.yaml`](docs/contracts/openapi.yaml) | 控制面 OpenAPI、DTO、鉴权、分页与错误响应 |
| [`docs/contracts/events.schema.json`](docs/contracts/events.schema.json) | WebSocket/Tauri 事件 envelope 与 payload |
| [`docs/DATABASE-SCHEMA.md`](docs/DATABASE-SCHEMA.md) | SQLite 表、约束、索引、加密边界与迁移规则 |
| [`docs/ERROR-CODES.md`](docs/ERROR-CODES.md) | 稳定错误码与 HTTP/重试映射 |
| [`docs/XRAY-MAPPING.md`](docs/XRAY-MAPPING.md) | 共享 Xray 引导、入站/出站、路由、`outbound_key` 与故障恢复 |
| [`docs/UI-SURFACES.md`](docs/UI-SURFACES.md) | Profiles、创建/详情、代理、内核、活动和三档运行模式 |
| [`docs/TEST-FIXTURES.md`](docs/TEST-FIXTURES.md) | 内核、代理、网络、Xray、模式、API 与数据库 fixture |
| [`docs/DELIVERY-PLAN.md`](docs/DELIVERY-PLAN.md) | M0 spike、第一个纵向切片和开工检查表 |
| [`docs/THIRD-PARTY.md`](docs/THIRD-PARTY.md) | 第三方组件、许可义务与当前合规缺口 |
| [`docs/RELEASE.md`](docs/RELEASE.md) | macOS/Docker 发布、AdHoc 签名、SBOM、门禁与回滚 |
| [`docs/RELEASE-POLICY.md`](docs/RELEASE-POLICY.md) | 手动 `release_target`、trusted baseline、reservation 与 recovery |
| [`docs/CI.md`](docs/CI.md) | GitHub Actions PR/main/release 拓扑、AdHoc 打包、门禁与发布环境 |
| [`docs/contracts/manifests/`](docs/contracts/manifests/) | 148 内核与 Xray v26.11.9 候选能力清单 |

## 主题规格

- [`docs/specs/profile-management/SPEC.md`](docs/specs/profile-management/SPEC.md)：环境档案、分组、导入导出与生命周期元数据。
- [`docs/specs/fingerprint-engine/SPEC.md`](docs/specs/fingerprint-engine/SPEC.md)：指纹规格、种子、一致性校验与启动参数生成。
- [`docs/specs/browser-runtime/SPEC.md`](docs/specs/browser-runtime/SPEC.md)：内核进程、有头/无头/伪有头模式、监督、崩溃恢复与显示环境。
- [`docs/specs/proxy-gateway/SPEC.md`](docs/specs/proxy-gateway/SPEC.md)：VLESS/VMess/Shadowsocks/Trojan/Hysteria2、协议引擎、分享链接、远程 DNS 与泄漏防护。
- [`docs/specs/control-api/SPEC.md`](docs/specs/control-api/SPEC.md)：控制面 HTTP/WS 契约、鉴权、事件与自动化入口。

## 关键产品约束

- 指纹内核不能凭空创造宿主不存在的显示属性。`screen.width`、`screen.height`、`devicePixelRatio` 与字体枚举仍受宿主显示器和已安装字体影响；跨操作系统画像只能做到尽力对齐，不能承诺与真实设备完全一致。
- 无头模式不是有头模式的等价替代。fingerprint-chromium 只处理了部分 Headless 特征，其余特征仍可能暴露；无头档案会被标记为“降低保证”。伪有头只把 `HeadlessChrome` UA 改为 `Chrome`，其他 Headless 特征仍保留。
- Chromium 的 `--proxy-server` 不能携带代理认证凭据，也不支持 VLESS/VMess/Shadowsocks 等协议。产品运行单实例共享 Xray 引擎，为每个运行档案创建回环 HTTP 入站和路由；相同节点/凭据复用出站与连接池，凭据不进入浏览器进程。
- Xray-core 以 MPL-2.0 分发。产品将其作为独立进程使用，发布物保留许可证与来源信息；共享引擎是网络单点，必须失败关闭并在崩溃后按原端口重建。
- 指纹浏览器提供环境隔离与指纹一致性，不提供匿名性保证，也不承诺绕过任何平台的风控或服务条款。
- 上游项目明确不提供技术支持，源码按延迟策略发布。内核缺陷只能通过版本准入、沙箱、功能降级或自建补丁分支应对。

## 开发入口

实现开始前应先确认 [`docs/PRD.md`](docs/PRD.md) 的范围与验收，再阅读 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)、[`docs/DESIGN.md`](docs/DESIGN.md) 与 [`docs/TECH-STACK.md`](docs/TECH-STACK.md) 建立工作区，并以 [`docs/specs/`](docs/specs/) 中的 `REQ-*` 与 `VER-*` 作为实现和验证的契约。
