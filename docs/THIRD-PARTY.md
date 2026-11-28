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
| Debian bookworm-slim（含 bash/coreutils 等 GPL-3.0 与 glibc 等 LGPL-2.1 包） | 固定 digest | Debian 与各包自身许可 | Docker 基础镜像 | 按实际安装包生成 SBOM，随镜像提供许可文本、来源与版本；对 GPL/LGPL 包提供精确版本的 Corresponding Source 获取 URL 与至少三年书面要约/随附源码，纳入 LICENSES、镜像与发布附件清单 |
| tini | 固定版本（镜像内） | MIT | Docker 运行时 init | 保留 MIT 文本与版权声明；源码链接见 §5 |
| Xvfb（xorg-server） | 固定版本（镜像内） | X11/MIT | Docker 运行时 | 保留版权与许可文本；源码链接见 §5 |
| openbox | 固定版本（镜像内） | GPL-2.0-or-later | Docker 运行时，独立进程 | 提供许可证文本、源码链接与 Corresponding Source；不链接到 Mien |
| x11vnc | 固定版本（镜像内） | GPL-2.0-or-later | Docker 运行时，独立进程 | 提供许可证文本、源码链接与 Corresponding Source；不链接到 Mien |
| noVNC | 固定版本（镜像内静态资源） | MPL-2.0（代码）以及随附的 BSD-3-Clause、BSD-2-Clause、OFL-1.1、CC-BY-SA-3.0、MIT 资源/字体许可 | Docker 运行时，默认关闭 | 保留 MPL-2.0 文本与来源；如修改 MPL 文件必须公开修改后的文件；随附字体、图标与其他资源分别保留 BSD-3-Clause、BSD-2-Clause、OFL-1.1、CC-BY-SA-3.0、MIT 文本与归属；`core/des.js` 的 BSD-style 声明必须随源码保留 |
| websockify | 固定版本（镜像内） | LGPL-3.0 | Docker 运行时，独立进程 | 保留 LGPL-3.0 文本；提供 Corresponding Source 与可替换方式；不修改或修改后公开 |
| 字体包 | 固定版本 | OFL-1.1、Apache-2.0 等按字体 | Docker 与桌面资源 | 只分发允许再分发的字体；逐字体记录许可与来源 |
| Moby seccomp 基线配置 | 提交 `a0fa7080` | Apache-2.0 | 仓库 `deploy/docker/seccomp/chromium.json`；宿主侧发布物分发，镜像内只保留审计副本 | 保留 Apache-2.0 文本与来源；文件内 `_notice` 记录修改；`PROVENANCE.json` 记录上游摘要与全部增量（`clone`/`unshare`、`mount`/`umount2`/`chroot`；`clone3` 保持基线） |

## 2. Copyleft 组件义务

openbox、x11vnc、websockify、noVNC 与 Xray-core 是独立程序，不作为库链接进 Mien 二进制：

- 每个组件以独立可执行文件或独立静态资源目录分发，Mien 进程不加载其符号，也不与其共享地址空间。Debian base image 中逐包分发的 GPL-3.0/LGPL-2.1 组件同样受源码获取义务覆盖，不能因来自基础镜像而排除。
- 发布说明、镜像与本文必须给出每个受 copyleft 组件的精确版本、许可文本、上游源码 URL 与 Corresponding Source 获取方式；GPL-2.0/GPL-3.0 组件提供不少于三年的书面源码要约或等价随附源码，LGPL-2.1/LGPL-3.0 提供相应源码和可替换方式。
- websockify 为 LGPL-3.0：不改名、不剥离许可信息；不修改时保持上游二进制/脚本原样分发并说明可替换方式；修改时公开修改文件与构建方式。
- noVNC 为 MPL-2.0：保留文件头与许可声明；修改的 MPL 文件必须按 MPL-2.0 公开。其发行包同时包含 BSD-3-Clause、BSD-2-Clause、OFL-1.1、CC-BY-SA-3.0 与 MIT 的字体/资源，且 `core/des.js` 带有 BSD-style 声明；`LICENSES/` 必须逐项收录，不能只列 MPL-2.0。
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

## 6. 自动生成与发布阻断

许可清单由 `cargo run -q -p xtask -- licenses` 生成，`--check` 只比较结构化输入与已提交产物，不下载网络内容；`xtask check` 已调用 `licenses --check`，因此 `NOTICE`、`LICENSES/` 与 `third-party-report.json` 漂移会使检查失败。生成器只接受报告中声明的白名单许可证：Cargo 链接依赖仅允许 `MIT`、`Apache-2.0`、`BSD-2-Clause`、`BSD-3-Clause`、`ISC`、`Unicode-3.0`、`Zlib` 及 `Apache-2.0 WITH LLVM-exception`，前端允许集合另加 `0BSD`、`CC0-1.0`、`Unlicense`、`BlueOak-1.0.0`，独立运行时组件按其实际分发许可收录 MPL/GPL/LGPL/OFL/CC-BY-SA；未知、缺失或白名单外许可证立即非零退出。

### 6.1 组件矩阵与证据

生成报告的每一行包含 `id`、`version`、`license`、`distribution`、`obligations`、`sourceUrl` 和 `evidencePaths`。版本来源固定为：fingerprint-chromium 与 Xray-core 取 `docs/contracts/manifests/*.json`，Rust 取 `Cargo.lock` 配合离线 `cargo metadata`，前端取 `bun.lock` 配合安装后的 `node_modules/<package>/package.json`（本机无 `node_modules` 时可使用 Bun cache；PR job 必须先 `bun install --frozen-lockfile`），容器组件取 `deploy/docker/`、`PROVENANCE.json` 与 release job 的镜像 SBOM；生成器不下载任何源码或许可证文本。

| 组件 | 版本/证据 | 许可证 | 分发形态 | 义务与源码获取 |
| --- | --- | --- | --- | --- |
| 项目 Mien | 仓库根 `LICENSE` | Apache-2.0 | 项目 | 随发布物分发完整 `LICENSE`；源码为本仓库对应发布 SHA。 |
| fingerprint-chromium | `148.0.7778.215`；各平台 kernel manifest | BSD-3-Clause | 独立静态目录/内核资产 | 保留版权和许可，不暗示背书；源码：<https://github.com/adryfish/fingerprint-chromium/tree/148.0.7778.215>。 |
| Xray-core | `v26.11.9`；Xray manifest | MPL-2.0 | 独立进程，不链接 | 保留许可和来源；未修改时提供上游 Corresponding Source，修改 MPL 文件时公开修改文件；源码：<https://github.com/XTLS/Xray-core/tree/v26.11.9>。 |
| Rust crates | `Cargo.lock` 精确版本 | 逐包 SPDX，限宽松白名单 | 链接依赖 | 保留选择的许可证文本；GPL/AGPL/SSPL、未知或缺失许可证阻断；源码 URL 来自 cargo metadata repository/crates.io。 |
| Bun 前端依赖 | `bun.lock` 精确版本 | 逐包 SPDX，限前端白名单 | 打包静态资源 | 保留包许可证与归属；源码 URL 来自 package metadata/npm fallback。 |
| Debian bookworm-slim | 发布镜像固定 digest + image SBOM | Debian 各包实际许可证 | 容器基础镜像 | release `docker-amd64` job 生成包级 SBOM、许可文本和源码 URL；GPL/LGPL 包提供 Corresponding Source 或至少三年书面要约。 |
| Xvfb/xorg-server | 容器包精确版本 | MIT/X11 | 独立进程 | 保留版权、许可和包级源码记录。 |
| openbox、x11vnc | 容器包精确版本 | GPL-2.0-or-later | 独立进程 | 保留 GPL 文本、源码 URL 与 Corresponding Source/书面要约。 |
| websockify | 容器包/源精确版本 | LGPL-3.0 | 独立进程 | 保留 LGPL 文本、源码和可替换方式；修改文件必须公开。 |
| noVNC | 容器源精确版本 | MPL-2.0；资源另含 BSD-3-Clause/BSD-2-Clause/OFL-1.1/CC-BY-SA-3.0/MIT | 独立静态目录 | 保留 MPL 文件头、资源/字体归属与 `core/des.js` BSD-style 声明；修改 MPL 文件必须公开。源码：<https://github.com/novnc/noVNC/tree/v1.6.0>（实际版本以镜像 SBOM/构建输入为准）。 |
| tini | 容器包精确版本 | Apache-2.0 | 独立进程 | 保留 Apache 文本和源码 URL：<https://github.com/krallin/tini>。 |
| Moby seccomp | commit `a0fa70805f742f7da15b367770d802ea2a86c68b`；`PROVENANCE.json` | Apache-2.0 | 独立宿主配置 | 保留上游文本、commit、修改记录和摘要；源码：<https://github.com/moby/profiles/tree/a0fa70805f742f7da15b367770d802ea2a86c68b>。 |

容器版本不得使用 `container-locked version` 作为最终发布证据：这是当前未处置项，必须由 `docker-amd64` 以固定基础镜像 digest、Dockerfile/安装包版本和镜像 SBOM 替换报告中的占位值，否则发布阻断。

### 6.2 NOTICE 与 LICENSES 目录约定

`NOTICE` 为生成文件，每个组件使用固定字段顺序：`Component`、`Version`、`License`、`Distribution`、`Source`、`Obligations`、`Evidence`；同一组件多许可证用 `; ` 分隔，证据路径始终为仓库相对路径。`LICENSES/<id>.txt` 的 `<id>` 是 SPDX identifier（例如 `Apache-2.0.txt`、`MPL-2.0.txt`、`BSD-3-Clause.txt`）；同一 SPDX 文本只存一份，发布 staging 必须从固定包/源码归档写入精确上游文本，不接受仅有 URL 的占位文本。不得提交第三方二进制，Xray/Chromium 仅以清单与 release 构建输入记录。

### 6.3 SBOM 与 job 责任

`contracts` job 不下载、不构建镜像，只执行 `cargo run -q -p xtask -- licenses --check` 和 `cargo run -q -p xtask -- contracts`；PR workflow 的 contracts job 应加入该命令（现有 T1 workflow 不在本任务修改，补丁建议见 §6.4）。Rust SBOM 由 `cargo sbom --output-format cyclonedx-json > SBOM/rust.cdx.json` 生成，若 runner 尚未安装 cargo-sbom 则由 release job fail closed；Bun SBOM 由 `syft dir:. --scope all-layers -o cyclonedx-json=SBOM/frontend.cdx.json` 在 `bun install --frozen-lockfile` 后生成；容器 SBOM 由 `syft <image>@<digest> -o cyclonedx-json=SBOM/container.cdx.json` 生成。macOS job 生成 Rust/Bun/application SBOM，docker-amd64 job 生成 container SBOM、包级许可和漏洞报告，stage-release 汇总为发布附件并校验摘要；不把许可证报告当成 SBOM 的替代品。

### 6.4 PR contracts job 补丁建议（不直接改 T1 workflow）

在 `.github/workflows/ci-reusable.yml` 的 `contracts` job、所有 `checkout` 与依赖安装步骤成功后加入：

```yaml
- name: Check third-party license inventory
  run: cargo run -q -p xtask -- licenses --check
```

该步骤只做结构化校验，不下载第三方内容；`xtask check` 本地同样执行。若要从干净 runner 生成而非校验，先执行 `bun install --frozen-lockfile`，再运行 `cargo run -q -p xtask -- licenses`，并将 `NOTICE`、`LICENSES/`、`third-party-report.json` 作为 job artifact；PR required gate 只接受 `licenses --check` 成功。

### 6.5 未处置即阻断发布清单

| 项目 | 证据/Owner | 状态 | 阻断条件 |
| --- | --- | --- | --- |
| 容器基础镜像 digest 与包级 SBOM | `docker-amd64` / Release owner | 未处置 | 缺 digest、包版本、许可证文本、源码 URL 或 GPL/LGPL Corresponding Source。 |
| 容器组件精确版本 | `deploy/docker/*` + image SBOM / T8 | 未处置 | 仍使用生成器的 `container-locked version` 占位值。 |
| 精确第三方许可证文本 | release staging / Legal owner | 未处置 | `LICENSES/*.txt` 仍含 “Canonical license text” provenance stub，或缺归属。 |
| 安全例外与未维护依赖 | `deny.toml`、`src-tauri/deny.toml` / T8 | 未处置 | `RUSTSEC-2026-0459`、time/quick-xml 与 6 条 unmaintained 仅是安全例外，不得当作许可证通过；未有接受记录、升级或移除即阻断。 |

当前判定命令：`cargo run -q -p xtask -- licenses --check && jq -e '.licenseStatus == "pass" and (.unresolvedLicenseItems | length == 0) and (.releaseBlockers | length == 0)' third-party-report.json`。当前报告必然为阻断状态，原因是容器证据、精确文本和已知安全例外尚未处置；即使许可证扫描为 pass，也不得发布。

## 7. 参考
