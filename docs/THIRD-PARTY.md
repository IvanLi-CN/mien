# Mien 第三方组件与许可

本文记录发布物中的第三方组件、许可义务和当前合规状态。它不是法律意见；发布前必须由负责人完成最终评审。项目自身以 Apache-2.0 发布，仓库根提供 `LICENSE`。

## 1. 运行时组件

| 组件 | 版本 | 许可 | 分发方式 | 义务 / 状态 |
| --- | --- | --- | --- | --- |
| fingerprint-chromium | 148.0.7778.215 | BSD-3-Clause | 随桌面应用和 Docker 镜像分发 | 保留版权与许可证；提供上游源码链接；不暗示上游背书 |
| Xray-core | v26.11.9 | MPL-2.0 | 独立进程分发，不链接 | 保留许可证与来源信息；提供 Corresponding Source；修改文件需按 MPL 披露；不暗示上游背书 |
| Tauri | 2.x | MIT 或 Apache-2.0 | Rust 依赖/桌面框架 | 保留依赖许可与 NOTICE |
| Rust crates | 锁文件版本 | MIT/Apache-2.0/BSD/ISC 等 | 链接依赖 | `cargo-deny` 检查；拒绝 GPL/AGPL/SSPL 进入链接依赖 |
| React / Vite / Radix 等前端依赖 | 锁文件版本 | MIT/Apache-2.0/ISC 等 | 打包静态资源 | 生成 SBOM；保留许可声明 |
| Debian bookworm-slim | 固定 digest | Debian 及各包许可 | Docker 基础镜像 | 镜像内保留许可信息；生成 SBOM；按包记录来源 |
| tini | 固定版本（镜像内） | MIT | Docker 运行时 init | 保留 MIT 文本与版权声明；源码链接见 §5 |
| Xvfb（xorg-server） | 固定版本（镜像内） | X11/MIT | Docker 运行时 | 保留版权与许可文本；源码链接见 §5 |
| openbox | 固定版本（镜像内） | GPL-2.0-or-later | Docker 运行时，独立进程 | 提供许可证文本、源码链接与 Corresponding Source；不链接到 Mien |
| x11vnc | 固定版本（镜像内） | GPL-2.0-or-later | Docker 运行时，独立进程 | 提供许可证文本、源码链接与 Corresponding Source；不链接到 Mien |
| noVNC | 固定版本（镜像内静态资源） | MPL-2.0 | Docker 运行时，默认关闭 | 保留 MPL-2.0 文本与来源；如修改文件必须公开修改后的文件 |
| websockify | 固定版本（镜像内） | LGPL-3.0 | Docker 运行时，独立进程 | 保留 LGPL-3.0 文本；提供 Corresponding Source 与可替换方式；不修改或修改后公开 |
| 字体包 | 固定版本 | OFL-1.1、Apache-2.0 等按字体 | Docker 与桌面资源 | 只分发允许再分发的字体；逐字体记录许可与来源 |
| Moby seccomp 基线配置 | 提交 `a0fa7080` | Apache-2.0 | 仓库 `deploy/docker/seccomp/chromium.json`；宿主侧发布物分发，镜像内只保留审计副本 | 保留 Apache-2.0 文本与来源；记录对默认 allowlist 的增量（`clone`/`unshare` 的 `CLONE_NEWUSER` 规则与 `clone3` 条件） |

## 2. Copyleft 组件义务

openbox、x11vnc、websockify、noVNC 与 Xray-core 是独立程序，不作为库链接进 Mien 二进制：

- 每个组件以独立可执行文件或独立静态资源目录分发，Mien 进程不加载其符号，也不与其共享地址空间。
- 发布说明和本文必须给出锁定版本的许可文本、上游源码 URL 与 Corresponding Source 获取方式；GPL-2.0 组件同时提供不少于三年的书面源码要约或等价随附源码的方式。
- websockify 为 LGPL-3.0：不改名、不剥离许可信息；不修改时保持上游二进制/脚本原样分发并说明可替换方式；修改时公开修改文件与构建方式。
- noVNC 为 MPL-2.0：保留文件头与许可声明；修改的 MPL 文件必须按 MPL-2.0 公开。
- Xray-core 为 MPL-2.0：不修改二进制；如启用补丁分支，必须建立可审计的补丁流程并发布修改后的源码。
- 不使用第三方名称暗示官方关联或背书。

### 2.1 已退出组件

- sing-box 不再作为运行时协议引擎；历史设计见 ADR-0007 与 proxy-gateway HISTORY。
- TUIC 不进入 v1；不得作为已支持协议宣传。
- 不引入内置代理、住宅 IP、短信、验证码或账号交易服务。

## 3. 发布义务与门禁

- 生成 SBOM（Syft）并随发布物提供；容器镜像同时输出包清单与被分发文件的许可信息。
- 发布物根目录包含 `LICENSE`（Apache-2.0）、`NOTICE`、`LICENSES/`（逐组件许可文本）与 `SBOM/`。
- 许可门禁：`cargo-deny`、前端依赖许可扫描、容器许可扫描和 SBOM 生成全部通过；出现未知许可、禁止许可或缺失许可文本时阻断发布。
- 发布说明列出：项目许可、第三方组件与版本、许可、源码获取方式、已知内核缺陷、无头/伪有头限制、TUIC 不支持、代理节点 bootstrap 解析与共享引擎故障域。
- 发布门禁同时要求 `deploy/docker/seccomp/chromium.json`、内核、Xray-core 与 SBOM 的 SHA-256 进入 SHA-256SUMS 与 attestation 主体；前端依赖与容器许可扫描失败时不得发布。
- 不使用第三方名称暗示官方关联或背书；不把 GPL 组件描述为 Mien 的一部分。

## 4. 发布前必须完成的合规动作

| 动作 | Owner | 证据 | 门禁 |
| --- | --- | --- | --- |
| 项目许可证落地 | Release owner | 仓库根 `LICENSE` 为 Apache-2.0，README 链接该文件 | M4 与每次发布 |
| 许可文本归档 | Release owner | `LICENSES/` 覆盖本文全部组件，`NOTICE` 列出归属 | 每次发布 |
| Copyleft 源码获取方式 | Legal owner | 发布说明包含固定版本源码 URL 与书面要约 | 每次发布 |
| 容器内 GPL/LGPL/MPL 组件评审 | Security owner | 组件清单、链接关系说明、源码与许可证据 | 每次发布 |
| Xray MPL-2.0 名称使用与再分发评审 | Legal owner | 评审记录与发布说明措辞 | 首次发布与重大变更 |
| 二进制签名链核验 | Release owner | 记录 Xray 与 fingerprint-chromium 发行资产的 SHA-256 与签名来源 | 每次内核/引擎升级 |
| 依赖与许可扫描 | CI owner | `cargo-deny`、`bun run license:check`、容器许可扫描与 SBOM 报告 | `CI PR / required`、`docker-amd64` 与发布 job |
| seccomp 基线来源与改动 | Security owner | 上游 commit、Apache-2.0 文本、`git diff` 摘要 | M0 与每次修改配置 |

## 5. 参考

- <https://github.com/adryfish/fingerprint-chromium>
- <https://github.com/adryfish/fingerprint-chromium/blob/148.0.7778.215/LICENSE>
- <https://github.com/XTLS/Xray-core>
- <https://github.com/XTLS/Xray-core/blob/v26.11.9/LICENSE>
- <https://github.com/krallin/tini>
- <https://gitlab.freedesktop.org/xorg/xserver>
- <http://openbox.org/>
- <https://github.com/LibVNC/x11vnc>
- <https://github.com/novnc/noVNC>
- <https://github.com/novnc/websockify>
- <https://github.com/moby/profiles/blob/a0fa70805f742f7da15b367770d802ea2a86c68b/seccomp/default.json>
- <https://github.com/moby/profiles/blob/a0fa70805f742f7da15b367770d802ea2a86c68b/LICENSE>
- <https://www.debian.org/legal/licenses/>
- [`contracts/manifests/`](contracts/manifests/)
- [`SECURITY.md`](SECURITY.md)
- [`TECH-STACK.md`](TECH-STACK.md)
