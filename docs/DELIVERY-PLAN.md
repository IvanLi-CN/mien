# Mien 交付计划：从文档到第一个纵向切片

本文把当前文档收敛成可执行的开工顺序。它不引入新架构决策；目标是在写生产代码前冻结契约、完成 M0 可行性验证，并用一个可演示的纵向切片证明核心路径。

## 1. 当前判断

- 产品方向、运行架构、Xray 共享引擎、运行模式和设计方向已经锁定。
- 仍缺少运行证据和若干机器契约；直接进入 M1 业务实现会放大返工风险。
- 可以立即开始 **M0 spike**；生产代码应在 M0 结论和契约冻结后开始。

## 2. M0：可行性与契约冻结

### 2.1 Xray 共享引擎 spike

目标：验证 `docs/XRAY-MAPPING.md` 的关键假设。

步骤：

1. 下载 Xray-core v26.11.9；校验 [`contracts/manifests/`](contracts/manifests/) 中的发行包 SHA-256。
2. 用 API 入站、block 出站和默认 block 路由启动引擎；API 只绑定 `127.0.0.1`。
3. 通过 `HandlerService` 添加一个 VLESS 出站、一个回环 HTTP 入站和一条路由规则。
4. 启动 fingerprint-chromium，通过该入站访问探针页；确认无直连回退。
5. 再启动一个使用相同节点/凭据的档案；确认只创建一个出站、连接池共享。
6. 启动一个使用不同凭据的档案；确认创建不同出站。
7. 强杀 Xray；确认所有运行档案失败关闭、产生事件，并按原端口重建。
8. 验证 Hysteria2（Tier B）：确认能力清单与失败标注正确，TUIC 被拒绝。
9. 从非回环地址访问 API 和入站；确认不可达。
10. 记录连接数、内存、启动耗时和重建耗时。

通过条件：

- `AddInbound` / `RemoveInbound` / `AddOutbound` / `RemoveOutbound` / `AddRule` / `RemoveRule` 可用。
- 相同 `outbound_key` 共享出站和连接；不同 key 不共享。
- 引擎崩溃后全部档案失败关闭，重建成功或明确失败。
- 入站/API 只绑定回环；TUIC 被拒绝；Hysteria2 结论明确。

证据：[`TEST-PLAN.md`](TEST-PLAN.md) 的 ENGINE-001–009、PROXY-003/005/009/013/014/027–029。

### 2.2 内核能力 spike

目标：验证 fingerprint-chromium 148 的实际能力，而不是只依赖 README。

步骤：

1. 校验 148 发行包和最终内核二进制 SHA-256。
2. 逐项验证 `--fingerprint`、平台、品牌、语言、时区、CPU、`--disable-spoofing`。
3. 分别运行有头、无头、伪有头；记录 UA、UA-CH 和其他 Headless 特征。
4. 验证 `headless_chrome_ua_patch` 是否只影响 UA。
5. 记录 canvas/WebGL 读回、字体、屏幕属性和已知 issue。

通过条件：

- 候选清单与实测一致；`pseudo_headful` 只声明 UA 范围。
- 有头与无头差异被记录，不宣称等价。
- 未知或无效开关不进入能力清单。

### 2.3 Docker 显示与沙箱 spike

目标：验证 `linux/amd64` 容器内的有头、无头、伪有头和查看器。

步骤：

1. 非 root 用户、只读根、`/data` 卷；启动 Xvfb + openbox。
2. 验证共享 `screen.*`、窗口尺寸、焦点和 noVNC 一次性令牌。
3. 验证 Chromium 沙箱状态；无法确认时拒绝启动。
4. 验证 Hysteria2 的 UDP 出站与容器网络。
5. 记录冷启动、内存、并发与 noVNC 故障行为。

通过条件：

- 有头模式可在无桌面容器内运行；无头和伪有头标记降低保证。
- 沙箱结论明确；查看器默认关闭且不暴露公网。
- 数据卷重启后保留数据，无孤儿进程。

### 2.4 契约冻结

M0 结束前必须冻结：

- [`contracts/openapi.yaml`](contracts/openapi.yaml)：端点、DTO、错误、鉴权、分页。
- [`contracts/events.schema.json`](contracts/events.schema.json)：事件枚举和 payload。
- [`ERROR-CODES.md`](ERROR-CODES.md)：稳定错误码与 HTTP 映射。
- [`DATABASE-SCHEMA.md`](DATABASE-SCHEMA.md)：表、约束、索引、迁移。
- [`XRAY-MAPPING.md`](XRAY-MAPPING.md)：出站/入站/路由、`outbound_key`、mux、DNS。
- [`contracts/manifests/`](contracts/manifests/)：内核与 Xray 候选清单。
- [`UI-SURFACES.md`](UI-SURFACES.md)：路由、表单、档案详情、代理、内核、活动。
- [`TEST-FIXTURES.md`](TEST-FIXTURES.md)：测试 fixture 目录。

任何契约变更必须同时更新对应 SPEC、测试和文档；不得只改代码。

首个纵向切片合并前，`.github/workflows/ci-pr.yml` 的 `CI PR / required` 必须已是真实 required check，且 `contracts` job 已执行本节的 OpenAPI/schema/DDL/链接/`spec_contract_check`；不得以“M0 先写实现、之后再接 CI”跳过契约门禁。

冻结后的契约必须在 CI 中通过 OpenAPI 生成差异、schema 编译、DDL 执行、内部 `$ref` 解析、链接检查与 `spec_contract_check`；任一检查失败即阻断合并。

### 2.5 许可评审

目标：在 M0 内闭合项目自身与第三方组件的许可义务，避免发布前返工。

步骤：

1. 确认项目以 Apache-2.0 发布；`LICENSE` 与 `README` 链接已就位，同一文本在 M4 随发布资产分发。
2. 评审 Xray-core 的 MPL-2.0：独立进程分发、不链接、保留许可证与来源信息、明确 Corresponding Source 的提供方式。
3. 评审内核的 BSD-3-Clause 与镜像内 GPL/LGPL 组件（Xvfb、openbox、x11vnc、tini、websockify、noVNC）的再分发义务。
4. 确定 `NOTICE`、`LICENSES/`、SBOM 与许可扫描的生成方式，并把未处置许可项作为发布阻断项。

通过条件：

- 项目许可、第三方许可与分发方式在 `docs/THIRD-PARTY.md` 中闭合，无未决许可问题。
- 发布物清单包含 `LICENSE`、`NOTICE`、`LICENSES/`、SBOM 与许可扫描结果。

## 3. M1：第一个纵向切片

目标：在 macOS 上创建档案、配置代理、启动有头浏览器、访问探针页、停止并再次启动。

顺序：

1. 建立 Rust workspace 与最小 Tauri/React 壳；不实现代理业务。
2. 实现 SQLite 迁移和 profile 领域模型；用 fixture 验证事务。
3. 实现指纹规格校验与确定性 argv；用 golden fixture 验证。
4. 实现 Xray 共享引擎 bootstrap、出站/入站/路由与引用计数。
5. 实现 macOS 有头启动、状态事件、停止和日志。
6. 接入 Profiles 首屏和创建/编辑表单；运行模式三档可见。
7. 用本地探针页验证指纹、代理、失败关闭和停止。
8. 增加无头/伪有头切换，但保持降低保证提示。

退出标准：

- 创建 → 配置代理 → 启动 → 探针 → 停止 → 再次启动成功。
- 相同规格重复启动 argv 一致；不同档案数据目录隔离。
- 共享引擎只有一个进程；相同出站 key 复用；停止后资源释放。
- 契约测试、SQLite 迁移、golden argv、OpenAPI/事件 schema 校验通过。
- 失败路径返回稳定错误码，不出现直连回退或凭据泄露。

## 4. 开工检查表

- [ ] M0 Xray spike 通过。
- [ ] M0 内核能力清单通过。
- [ ] M0 Docker 显示/沙箱通过。
- [ ] OpenAPI、事件 schema、错误码、数据库 schema 冻结。
- [ ] Xray 映射与 `outbound_key` 向量冻结。
- [ ] 内核与 Xray 清单校验通过。
- [ ] Profiles/创建/详情/代理/内核/活动 surface 边界确认。
- [ ] 测试 fixture 目录与所有权确认。
- [ ] 项目以 Apache-2.0 发布，`LICENSE` 与 `README` 链接就位，MPL-2.0/BSD-3-Clause 与容器组件许可义务闭合（M4 发布门禁复核）。
- [ ] 第一个纵向切片验收标准确认。

## 5. 不阻塞开工的事项

以下可以在 M1/M2 并行推进，不阻塞第一个切片：

- GitHub Actions CI/release workflow、AdHoc 打包、SBOM/许可构建。
- 10 档案容量与 24 小时浸泡。
- noVNC 之外的远程查看增强。
- 订阅、节点测速、多节点调度和 TUIC。
- 完整本地化文案、图标和所有响应式视觉证据。

## 6. 参考

- [`ROADMAP.md`](ROADMAP.md)
- [`PRD.md`](PRD.md)
- [`TEST-PLAN.md`](TEST-PLAN.md)
- [`SECURITY.md`](SECURITY.md)
- [`RELEASE.md`](RELEASE.md)
- [`RELEASE-POLICY.md`](RELEASE-POLICY.md)
- [`CI.md`](CI.md)
