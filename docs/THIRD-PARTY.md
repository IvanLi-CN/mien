# Mien 第三方组件与许可

本文记录发布物中的第三方组件、许可义务和当前合规状态。它不是法律意见；发布前必须由负责人完成最终评审。

## 1. 运行时组件

| 组件 | 版本 | 许可 | 分发方式 | 义务 / 状态 |
| --- | --- | --- | --- | --- |
| fingerprint-chromium | 148.0.7778.215 | BSD-3-Clause | 随桌面应用和 Docker 镜像分发 | 保留版权与许可证；提供上游源码链接；不暗示上游背书 |
| Xray-core | v26.11.9 | MPL-2.0 | 独立进程分发，不链接 | 保留许可证与来源信息；提供 Corresponding Source；修改文件需按 MPL 披露；不暗示上游背书 |
| Tauri | 2.x | MIT 或 Apache-2.0 | Rust 依赖/桌面框架 | 保留依赖许可与 NOTICE |
| Rust crates | 锁文件版本 | MIT/Apache-2.0/BSD/ISC 等 | 链接依赖 | `cargo-deny` 检查；拒绝 GPL/AGPL/SSPL 进入链接依赖 |
| React / Vite / Radix 等前端依赖 | 锁文件版本 | MIT/Apache-2.0/ISC 等 | 打包静态资源 | 生成 SBOM；保留许可声明 |
| Debian bookworm-slim | 固定 digest | Debian 及各包许可 | Docker 基础镜像 | 镜像内保留许可信息；生成 SBOM |
| Xvfb / openbox / x11vnc / noVNC | 固定版本 | X11/MIT/GPL 等按组件 | Docker 运行时 | 逐组件核对；GPL 组件作为独立系统工具使用，不链接到 Mien |
| 字体包 | 固定版本 | 各字体许可 | Docker 与桌面资源 | 只分发允许再分发的字体；记录许可 |

## 2. 已退出组件

- sing-box 不再作为运行时协议引擎；历史设计见 ADR-0007 与 proxy-gateway HISTORY。
- TUIC 不进入 v1；不得作为已支持协议宣传。
- 不引入内置代理、住宅 IP、短信、验证码或账号交易服务。

## 3. 发布义务

- 生成并随发布物提供 SBOM。
- 保留 fingerprint-chromium 的 BSD-3-Clause 文本与版权声明。
- 保留 Xray-core 的 MPL-2.0 文本、来源 URL 和 Corresponding Source 获取方式。
- 不修改 Xray 二进制；如需补丁，必须先建立可审计的补丁分支与源码发布流程。
- 容器内 GPL 组件只作为独立可执行工具使用，不链接到 Mien；发布说明列出组件与许可。
- 发布说明包含已知内核缺陷、无头/伪有头限制、TUIC 不支持、代理节点 bootstrap 解析和共享引擎故障域。
- 不使用第三方名称暗示官方关联或背书。

## 4. 当前缺口

- 仓库尚未选择 Mien 自身的项目许可证；这是发布前必须由项目负责人决定的事项。
- 尚未生成实际的 `LICENSES/`、`NOTICE` 和 SBOM 文件。
- 尚未完成 Xray MPL-2.0、fingerprint-chromium BSD-3-Clause 和容器内 GPL 组件的正式法律评审。
- 尚未验证 Xray 发行包与 fingerprint-chromium 发行包的最终二进制签名链；当前只记录发布资产 SHA-256。

## 5. 参考

- <https://github.com/adryfish/fingerprint-chromium>
- <https://github.com/adryfish/fingerprint-chromium/blob/148.0.7778.215/LICENSE>
- <https://github.com/XTLS/Xray-core>
- <https://github.com/XTLS/Xray-core/blob/v26.11.9/LICENSE>
- [`contracts/manifests/`](contracts/manifests/)
- [`SECURITY.md`](SECURITY.md)
