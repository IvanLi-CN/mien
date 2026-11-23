# 能力清单与发布资产

本目录记录首个发布候选的内核与 Xray 能力清单。JSON 文件使用 [`../kernel-manifest.schema.json`](../kernel-manifest.schema.json) 和 [`../proxy-engine-manifest.schema.json`](../proxy-engine-manifest.schema.json) 的结构。

## 1. 校验和语义

清单中的 `sha256` 是**公开发行包**的 SHA-256，来自对应 GitHub Release asset 的 `digest` 字段：

| 资产 | SHA-256 |
| --- | --- |
| fingerprint-chromium 148 macOS ARM64 DMG | `b72f091e2e1a7583eed389c4b8e3534ed355e568af8c8bbf8fc30a25e23ca679` |
| fingerprint-chromium 148 Linux x86_64 TAR.XZ | `70d239830332e5820aa34dfcb284161cac0429eee25da642830afe04bda717f4` |
| fingerprint-chromium 148 Windows x64 ZIP | `9ef3f471b7a6641b4224532522b29141ce3746e27d55788d88e2fd951f362579` |
| Xray-core v26.11.9 macOS ARM64 ZIP | `233f0c66681cd0726f81a6b2b6ed6a5710869e312047b522e46ed53035bebd26` |
| Xray-core v26.11.9 Linux x86_64 ZIP | `b651f7e1d9966ee2c1b1ee1bf8b52c72771824d3065dce96bbc8bcc19ed95f82` |

安装流程必须：

1. 下载发行包并校验上表 SHA-256；
2. 解压到版本目录；
3. 对最终可执行文件/内核二进制再次计算 SHA-256，并记录到实例的安装记录；
4. 不把清单中的发行包校验和冒充为最终二进制校验和。

## 2. 能力状态

- `headless_chrome_ua_patch: true` 来自 fingerprint-chromium 148 源码中的 `patches/extra/fingerprint/010-headless.patch` 与 `002-user-agent-fingerprint.patch`：它只把 `HeadlessChrome` UA 改为 `Chrome`，不改变其他 Headless 特征。
- `connection_reuse: true` 表示 Xray 支持 mux/原生 QUIC 连接复用；具体协议组合和共享出站行为仍需 M0 runtime spike 确认。
- 清单是**候选清单**，不是运行时准入通过证明。完成 Xray、内核、Docker 和协议矩阵 spike 后，必须用实测结果更新 `limitations` 与 `known_issues`。

## 3. 清单文件

- [`fingerprint-chromium-148.0.7778.215-macos-arm64.json`](fingerprint-chromium-148.0.7778.215-macos-arm64.json)
- [`fingerprint-chromium-148.0.7778.215-linux-x86_64.json`](fingerprint-chromium-148.0.7778.215-linux-x86_64.json)
- [`fingerprint-chromium-148.0.7778.215-windows-x64.json`](fingerprint-chromium-148.0.7778.215-windows-x64.json)
- [`xray-core-v26.11.9-macos-arm64.json`](xray-core-v26.11.9-macos-arm64.json)
- [`xray-core-v26.11.9-linux-x86_64.json`](xray-core-v26.11.9-linux-x86_64.json)

## 4. 更新规则

- 清单变更必须同时更新 SHA-256、`limitations` 和相关测试。
- 不得删除已发布清单中的已知缺陷；修复后在新版本清单中标记，并保留旧版本用于回滚。
- 不得把 TUIC 加入 v1 Xray 清单。
- 不得在清单或日志中写入代理凭据。
