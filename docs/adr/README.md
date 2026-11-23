# 架构决策记录

本目录记录难以逆转、存在真实取舍的架构与边界决策。每条 ADR 描述背景、决策、备选方案、后果和证据；主题规格引用相关 ADR，但不复制完整取舍理由。

| 编号 | 决策 | 状态 |
| --- | --- | --- |
| [ADR-0001](0001-shared-core-two-hosts.md) | 共享 Rust 核心与双宿主：Tauri 桌面 + HTTP 服务端 | 已接受 |
| [ADR-0002](0002-pin-kernel-148.md) | 固定 fingerprint-chromium 148 并建立内核准入 | 已接受 |
| [ADR-0003](0003-storage-and-secret-protection.md) | SQLite 存储与字段级密钥保护 | 已接受 |
| [ADR-0004](0004-local-proxy-relay.md) | 每档案本地代理中继 | 已被 ADR-0007 取代 |
| [ADR-0005](0005-headful-first-display.md) | 有头优先的显示策略 | 已接受 |
| [ADR-0006](0006-fingerprint-spec-model.md) | 显式指纹规格与种子派生模型 | 已接受 |
| [ADR-0007](0007-protocol-engine-forward-proxy.md) | 每档案独立协议引擎承载正向代理 | 已被 ADR-0009 取代 |
| [ADR-0008](0008-tauri-ipc-ui-transport.md) | 管理网页通过 Tauri IPC 调用核心 | 已接受 |
| [ADR-0009](0009-shared-xray-engine.md) | 单实例 Xray 动态协议引擎与连接复用 | 已接受 |
| [ADR-0010](0010-adhoc-macos-distribution.md) | macOS AdHoc 分发与 GitHub Actions 发布 | 已接受 |

## 阅读顺序

1. 先读 [`ADR-0001`](0001-shared-core-two-hosts.md) 理解桌面与服务端为什么共享核心。
2. 再读 [`ADR-0002`](0002-pin-kernel-148.md) 理解内核版本、能力清单和升级治理。
3. [`ADR-0006`](0006-fingerprint-spec-model.md) 与 [`ADR-0009`](0009-shared-xray-engine.md) 分别定义指纹输入和网络输入；[`ADR-0007`](0007-protocol-engine-forward-proxy.md) 与 [`ADR-0004`](0004-local-proxy-relay.md) 仅保留为被取代的历史背景。
4. [`ADR-0003`](0003-storage-and-secret-protection.md)、[`ADR-0005`](0005-headful-first-display.md) 与 [`ADR-0008`](0008-tauri-ipc-ui-transport.md) 补充持久化、密钥、显示和桌面调用边界。
5. [`ADR-0010`](0010-adhoc-macos-distribution.md) 规定 macOS AdHoc 分发、Gatekeeper 限制与 GitHub Actions 发布边界。

新的 ADR 使用四位编号和 kebab-case 文件名；语义变更通过后继 ADR 取代旧决策，不直接改写已接受决策的理由。
