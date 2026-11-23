# Mien 测试计划

## 1. 目标与原则

测试计划验证产品是否满足 [`PRD.md`](PRD.md) 的验收门槛，重点是四类风险：指纹内部不一致、代理或网络泄漏、运行时生命周期失控、控制面认证与敏感数据泄露。测试以自有探针和本地可控服务为主，第三方指纹站点只作为辅助证据，避免外部页面变化或已知内核兼容问题阻断发布。

原则：

- 先建立可重复的本地 fixture，再引入外部站点。
- 同一结论至少有一个自动化证据；人工探索只补充视觉和交互问题。
- 指纹测试区分显式字段、种子派生字段和宿主绑定字段，不能把宿主差异误判为产品缺陷。
- 代理测试必须有失败关闭和泄漏 canary，不能只看“能上网”。
- 安全测试覆盖攻击路径，而不只检查配置项。
- 发布门禁不可整类豁免；P0 与指定的 P1 类别零容忍，缺失证据等同未通过。
- 发布门禁以本文的用例和 [`PRD.md`](PRD.md) 的 PRD-ACC 编号为准。

## 2. 测试环境

| 环境 | 配置 | 用途 |
| --- | --- | --- |
| macOS 桌面 | Apple Silicon，macOS 13+，干净用户账户 | 桌面宿主、钥匙串、有头运行、AdHoc 打包与完整性 |
| Docker x86_64 | Linux x86_64，8 vCPU / 16 GB，非 root 容器 | 服务端、Xvfb 有头、无头、代理与容量 |
| Docker on Apple Silicon | Docker Desktop 或 OrbStack，`linux/amd64` 模拟 | 兼容性与性能记录，不作为性能基线 |
| HTTP/SOCKS fixture | 本地 HTTP、HTTPS、SOCKS5 代理，支持认证与 DNS 日志 | 基础代理、认证、远程 DNS、失败关闭 |
| 协议节点 fixture | 受控 Tier A（VLESS、VMess、Shadowsocks、Trojan）与 Tier B（Hysteria2、WireGuard）服务端，支持 TLS/REALITY/WS/gRPC/QUIC 组合 | 协议矩阵、分享链接、引擎配置、凭据隔离 |
| DNS canary | 本地权威 DNS 或可控解析日志 | 检测本地 DNS 泄漏 |
| 目标站点 fixture | 本地静态站点、TLS 站点、双栈站点、WebRTC 测试页 | 指纹探针、IPv6、WebRTC |
| 外部站点 | CreepJS、BrowserLeaks、BrowserScan、PixelScan | 辅助观察，不作为唯一门禁 |

内核固定为 fingerprint-chromium `148.0.7778.215`，协议引擎固定为构建清单中的 Xray-core 版本。任何候选内核或引擎必须先在相同环境跑完整准入套件，才能进入产品测试矩阵。

## 3. 测试层级

| 层级 | 范围 | 工具 | 频率 |
| --- | --- | --- | --- |
| 单元 | 指纹校验、argv 构建、路径规范化、错误映射、加密 | Rust 测试 | 每次提交 |
| 契约 | OpenAPI、事件 schema、导入导出 schema | schema 校验与生成客户端 | 每次提交 |
| 集成 | SQLite、档案、代理配置、协议引擎生命周期、运行时状态机、API 鉴权 | 测试容器与本地进程 | 每次合并 |
| 端到端 | macOS 与 Docker 的完整启动、指纹探针、停止 | Playwright/CDP、容器脚本 | 每日与发布前 |
| 指纹 | 同档案稳定性、跨档案差异、画像一致性 | 自有探针 | 每日与内核变更 |
| 安全 | 认证、CSRF、密钥、路径、沙箱、镜像 | 安全用例与扫描器 | 每次合并与发布前 |
| 视觉/UI | 首屏空间布局、响应式、键盘、双语、状态语义 | Playwright 截图与人工检查 | 每次 UI 变更与发布前 |
| 性能/浸泡 | 启动耗时、并发、内存、文件描述符 | 压测脚本与监控 | 发布前 |
| 打包 | AdHoc 签名、镜像、SBOM、健康检查 | 平台工具 | 发布前 |

### 3.1 UI 与视觉验收

视觉基线为 [`../DESIGN.md`](../DESIGN.md) 的 Deep Current 规则与 [`design/profiles-dark.png`](design/profiles-dark.png) 的首屏 comp。视觉验收与功能验收分离：截图不能替代指纹、代理和生命周期证据，但布局、状态语义、可访问性和限制披露必须在发布前可见。

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| UI-001 | 1440×900 首屏结构 | Space 侧轨、命令栏、Space 标题、Live session 面板与 profile 字段完整；无表格、无常驻右侧检查器；名称、状态、代理与 proof 无需 hover 即可见 |
| UI-002 | 响应式 | 1024、768、390 宽度无横向溢出；侧轨按契约折叠；主操作可达；卡片不缩成不可读的网格 |
| UI-003 | 状态与隔离语义 | 六态 `starting`/`running`/`stopping`/`stopped`/`crashed`/`failed` 同时有文本、圆点或 proof 与颜色；`isolation` 的 `unknown` 与 `stopped` 在语义和视觉上可区分（`unknown` 虚线描边加 `?`，`stopped` 低透明度中性，不得共用同一呈现）；颜色不是唯一信号 |
| UI-004 | 键盘与动效 | 主要操作、浮层和对话框可键盘完成；焦点可见；`prefers-reduced-motion` 下关闭抬升与呼吸动画 |
| UI-005 | 双语 | 中英文切换不破坏层级；中文标签无强制大写或截断；档案语言、时区与界面语言相互独立；标识与度量保持等宽 |
| UI-006 | 限制披露 | host-bound 显示、字体与 plugins 限制、无头降级、`isolation` 为 `unsandboxed` 或 `unknown` 时的降级标记与 fail-closed 状态在界面可见；停止的档案仍显示最近一次运行的沙箱状态与降低保证，从未启动且无运行记录时显示 `unknown`；不出现“不可检测”“绝对安全”等承诺 |
| UI-007 | 视觉系统 | 身份渐变只用于 Space、orb 与 aura；状态色只用于状态；无玻璃、霓虹、纯黑纯白大面积底色或等宽四列卡片墙 |
| UI-008 | 运行模式选项 | 模式选项显示“有头 / 无头 / 伪有头”；无头与伪有头显示降低保证；伪有头明确只覆盖 UA，不暗示有头等价 |
| UI-009 | `/profiles/new` 表单状态 | 加载/重复提交阻止；字段错误定位；未保存离开确认；凭据不回显；能力缺失时保存/启动被阻断且原因可见；代理检查失败只显示警告与 proof 变化，界面不提供“检查失败禁止启动”选项；重复 start 只显示进行中状态并复用现有运行时 |
| UI-010 | `/profiles/:id` 详情状态 | 四段 proof（含 `isolation` 三值）、`reduced_assurance`、`ua_scope`、六态、启动/停止/取消、`crashed`/`failed` 重试、共享引擎故障与重建失败、日志尾部、归档/恢复/删除/`deletion_error` 重试入口可见；终态后不显示残留内核；不需要打开详情才能判断哪项失败 |
| UI-011 | `/proxies` | 类型化字段、TUIC 拒绝、分享链接只生成草稿、`credential_set` 不回读、`RESOURCE_IN_USE` 提示、检查错误类可区分 |
| UI-012 | `/kernels` | 展示 `flags`、`headless_chrome_ua_patch`、已知问题、引擎 license/source_url；切换确认和连接池重置提示 |
| UI-013 | `/activity` | `runtime.*`/`proxy.*` 精确事件名；按 `event_type`、`profile_id`、`action`、`from`/`to` 筛选，与审计契约字段一致；组合筛选与分页无重复遗漏；空状态和脱敏可见 |
| UI-014 | 导入/导出 | Import 入口、明文/加密选择、口令、凭据移除提示、冲突处理和导出动作可用；错误字段可定位 |
| UI-015 | 可访问性执行与证据 | 每个 surface 通过 axe 或等价扫描并归档报告；对比度阈值、角色/名称、200% zoom、禁用原因、焦点顺序和 reduced motion 有断言；缺少扫描报告或断言失败即视为该 surface 未完成 |
| UI-016 | `/profiles/:id/edit` 状态 | 加载、保存中、字段错误、未保存离开确认、凭据只显示“已设置”、导入/导出入口与口令字段状态、检查失败只作警告、归档/恢复/删除/`deletion_error` 动作可用且原因可定位 |
| UI-017 | `/settings` 状态 | 加载、空、错误与重试；Token 只显示一次且可撤销；查看器开关与 `VIEWER_NOT_AVAILABLE` 提示；服务端部署限制可见；桌面端不显示服务端专属项且无本地 HTTP 端口 |
| UI-018 | 列表状态 | 首屏 loading 骨架、空空间、筛选无结果、加载失败与重试、分页或滚动加载；空状态说明原因，不显示空白表格或无声空白区域 |
| UI-019 | 插件与宿主绑定 | `plugins`、字体、`screen.*`、`devicePixelRatio` 标记为 host-bound；界面不承诺其随画像变化；报告分类与 UI 标注一致 |
| UI-020 | 外部站点仅辅助 | CreepJS、BrowserLeaks 等第三方结果只出现在辅助说明或报告中，不作为 UI 判定、proof 通过条件或门禁证据；界面不出现“已通过某第三方站点”的结论 |
| UI-021 | 日志尾部 | 详情页日志来自有界日志端点或等价桌面 IPC；显示脱敏内容、`limit` 与截断标记；端点不可用时显示稳定错误码与重试，不留空白 |
| UI-022 | 生命周期动作 | 归档→恢复、删除到回收站→恢复、彻底删除可用；`deleted` 档案不可启动且按钮禁用原因可见；彻底删除失败显示 `deletion_error` 与重试 |

## 4. 内核准入

候选内核必须通过以下套件，结果按内核版本和平台归档。

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| KERNEL-001 | 有头启动与退出 | macOS 与 Docker 均能启动、加载本地页面、正常退出 |
| KERNEL-002 | canvas 读回 | `getImageData` 与 WebGL `readPixels` 在常见格式下不崩溃；子区域与完整读回差异记录 |
| KERNEL-003 | 真实站点加载 | 至少 5 个重前端站点可加载，无 renderer crash；x.com 作为已知回归站点 |
| KERNEL-004 | 指纹探针 | UA、UA-CH、平台、时区、语言、WebGL、canvas、音频、WebRTC 字段可采集 |
| KERNEL-005 | 种子稳定性 | 同种子同参数三次启动的派生哈希一致 |
| KERNEL-006 | 跨种子差异 | 20 个种子的 canvas、音频与 WebGL 哈希至少 95% 不同 |
| KERNEL-007 | 能力清单 | 文档化开关逐项验证；无效果或未验证开关不进入清单 |
| KERNEL-008 | 沙箱 | 目标容器配置下沙箱状态可确认；无沙箱路径有显式开关与审计 |
| KERNEL-009 | 稳定性 | 100 次启动/停止无孤儿；30 分钟连续浏览无崩溃 |
| KERNEL-010 | 安全回归 | 已知越界访问报告有复现或缓解结论；无法缓解时阻断发布 |

已知 148 问题：CreepJS 在部分 Windows 环境可能卡住（#108）；canvas 子区域与完整读回可能不一致（#98）；WebGL readPixels 在 1×1 与 8×8 读回间可能不一致（#99）；WebGL readPixels 打包 16 位格式存在越界访问（#102）。已知 150 问题：canvas 读回崩溃与站点加载失败（#94、#95、#104、#112）。测试报告必须记录这些结论，不能把第三方站点卡住直接归因于产品。

## 5. 指纹探针与判定

### 5.1 采集字段

| 分类 | 字段 | 说明 |
| --- | --- | --- |
| 显式 | `navigator.userAgent`、`navigator.platform`、`navigator.userAgentData`、`navigator.language(s)`、`Intl.DateTimeFormat().resolvedOptions().timeZone`、`navigator.hardwareConcurrency` | 与规格比对 |
| 派生 | canvas 2D 哈希、WebGL 图像哈希、WebGL vendor/renderer、音频哈希、`navigator.deviceMemory`、clientRects | 同种子同内核稳定；跨种子不同 |
| 宿主绑定 | `navigator.plugins`/`mimeTypes`、`screen.width/height`、`availWidth/height`、`devicePixelRatio`、`colorDepth`、字体枚举 | 只记录，不承诺按画像变化；`navigator.plugins` 由宿主安装的插件面派生，不得当作跨种子差异证据 |
| 自动化 | `navigator.webdriver`、CDP 相关特征 | 记录并标注限制 |
| 网络 | WebRTC 候选、代理出口 IP、时区与地区 | 与代理配置比对 |

### 5.2 判定

- **稳定性**：同一档案连续启动 3 次，显式字段完全一致，派生哈希完全一致；失败即 P1。
- **差异性**：20 个不同种子档案，canvas、音频、WebGL 哈希各自至少 95% 不同；低于阈值需调查种子或内核问题。
- **一致性**：平台、UA、UA-CH、品牌版本、语言、时区内部一致；不一致为 P1。
- **主机对齐**：默认预设的平台与宿主一致；跨系统画像必须带降低保证标记。
- **宿主泄漏**：Docker 中声明 Windows 画像时，记录字体、屏幕、插件面与图形栈泄漏；这些是已知限制，不因存在而阻断发布，但必须在 UI 与报告中可见。
- **插件分类**：`navigator.plugins` 与 `mimeTypes` 归为宿主绑定；指纹报告和 UI 不得把它们算作种子派生字段，差异性阈值统计排除该字段。
- **无头对比**：同一档案有头与无头各启动一次，记录差异；无头必须带降低保证标记，不得宣称等价。

### 5.3 探针用例

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| FP-001 | 同档案三次启动 | 显式字段与派生哈希一致 |
| FP-002 | 20 个种子差异 | 三类派生哈希差异率均 ≥95% |
| FP-003 | 平台与 UA/UA-CH 一致 | 无字段矛盾 |
| FP-004 | 品牌与版本 | 品牌版本不高于内核主版本，平台支持 |
| FP-005 | 语言与时区 | 与规格一致；代理地区冲突有警告 |
| FP-006 | `--disable-spoofing` | 逐类关闭后对应探针变化，其他类别不受影响 |
| FP-007 | 宿主绑定字段 | 屏幕、DPR、颜色深度、字体按宿主记录并标记 |
| FP-008 | 无头标记 | API 与 UI 显示降低保证；有头不被静默降级 |
| FP-009 | 子区域读回 | 记录与完整读回差异，不崩溃 |
| FP-010 | 设备内存 | 记录内核返回的 8/16/32 与已知不常见值的差异 |
| FP-011 | DoH/IPv6 防护 | 启动后 DoH 不绕过代理、IPv6 直连被阻止；能力缺失或探针失败时拒绝启动 |
| FP-012 | 非代理 UDP 关闭审计 | 默认阻止；无高级确认写入被拒绝；确认后产生 `proxy.non_proxied_udp.disable` 审计 |
| FP-013 | 插件字段分类 | `navigator.plugins`/`mimeTypes` 记录为 host-bound；跨种子差异率统计不包含该字段；UI 与报告标注一致，无“插件随画像变化”的描述 |

## 6. 运行时与生命周期

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| RT-001 | 创建、启动、访问、停止 | 状态与事件正确，数据落盘 |
| RT-002 | 100 次启动/停止 | 无孤儿、无端口泄漏、无锁残留 |
| RT-003 | 强杀内核 | 状态变为 crashed，记录退出码与日志尾部 |
| RT-004 | 控制面强杀并重启 | 对账正确，可归属进程接管或安全停止，无重复 |
| RT-005 | 容器重启 | 数据保留，状态回到 stopped，无孤儿 |
| RT-006 | 并发启动 10 个档案 | 全部成功或按配额排队，无交叉数据目录 |
| RT-007 | 达到配额与低内存 | 拒绝或排队，返回稳定错误，不部分启动 |
| RT-008 | 停止超时与资源清理 | 进程组被终止；运行档案入站/规则删除并清空 `proxy_outbound_key`、`proxy_inbound_tag`、`proxy_rule_tag`、`proxy_port`；共享出站引用仅在非终态运行时计数，归零后删除；共享引擎继续运行；锁释放 |
| RT-009 | CDP 暴露 | 回环可用，容器外/局域网不可达；关闭时无端口 |
| RT-010 | Xvfb/noVNC | 有头可运行；查看器默认关闭；一次性令牌失效；查看器故障不影响浏览器 |
| RT-011 | 沙箱 | 严格模式启动成功；无法确认时拒绝；显式无沙箱有审计 |
| RT-012 | 内核校验 | 篡改/缺失内核拒绝启动，无残留进程 |
| RT-013 | 日志轮转与脱敏 | 单文件 10 MB、最多 5 个；无密码/Token/Cookie |
| RT-014 | 健康与就绪 | 依赖缺失时就绪失败，恢复后成功 |
| RT-015 | 运行模式矩阵 | 有头、无头、伪有头可分别启动；后两者返回 `reduced_assurance=true`；伪有头返回 `mode=pseudo_headful` 与 `ua_scope=chrome`；不静默降级 |
| RT-016 | 伪有头能力门禁 | `headless_chrome_ua_patch=false` 或缺失时拒绝启动并返回稳定错误；`true` 时只改变 UA，其他 Headless 特征仍被记录 |
| RT-017 | 同档案并发启动/停止 | 并发 start 只产生一个运行时与一个内核，重复请求返回进行中而非第二个实例；start 与 stop 交错不产生孤儿内核、重复入站或锁残留；并发 stop 幂等，终态稳定 |
| RT-018 | 跨档案数据隔离 | 两个档案并发运行时 Cookie、localStorage、缓存、会话与下载文件互不可见；默认下载目录为 `<data-root>/profiles/<id>/downloads`，权限 0700，路径包含校验拒绝越界 |
| RT-019 | 归档/删除/恢复与清理 | `active <-> archived` 通过 archive/restore 切换且不移动目录；`DELETE permanent=false` 将目录移到 `<data-root>/trash/<id>` 并写入 `deleted_at`，`deleted` 不可启动；恢复后目录回位且 `archived_at` 清空；`permanent=true` 先删目录再删行，失败保留 `deleted` 并写 `deletion_error` 供重试 |
| RT-020 | 崩溃循环 | 默认不自动重启；启用自动重启时最多 3 次递增退避，稳定运行后计数重置；超过上限进入 `crashed` 并停止重试；事件、退出码与日志尾部足以诊断，无无限重启 |
| RT-021 | 共享引擎故障与重建失败 | 运行中强杀引擎时全部运行档案网络失败且无直连/其他节点回退，产生 `proxy.engine.failed`；重建成功产生 `proxy.engine.rebuilt` 并恢复原端口；重建失败先转入 `stopping` 并拆除内核与进程组、入站/规则、出站引用、配额与锁，再写 `failed`；teardown 失败保持 `stopping` 与 `last_error`，终态后无残留内核或端口 |
| RT-022 | 沙箱校验先于资源创建 | 启动事务先校验沙箱，失败返回 `SANDBOX_UNAVAILABLE`，且不创建入站/出站/规则、不拉起进程、不占用配额；显式无沙箱时写入审计与降级标记，`isolation=unsandboxed` |
| RT-023 | 控制面重启对非终态运行时对账 | 重启后对 `starting`/`stopping` 行终止可归属内核、删除入站/规则/出站引用，再置 `stopped`；仅在拆除全部完成后才允许写 `failed`；不产生重复运行时，不可归属进程按契约记录而不盲杀 |
| RT-024 | 隔离 proof 状态 | 启动时沙箱探测写入 `runtimes.isolation`（`enforced`/`unsandboxed`/`unknown`）；proof strip 以该字段为唯一数据源派生，Profile payload 暴露同一派生值（取最近一次运行，无记录为 `unknown`）；停止的档案仍显示最近一次运行的 `unsandboxed` 与降低保证；`unknown` 与 `stopped` 不共用同一视觉或语义 |
| RT-025 | 日志尾部端点 | `GET /api/v1/profiles/{id}/logs?limit=` 从运行时日志环或轮转文件返回有界、脱敏日志；超出上限返回 422，未知档案 404，未认证 401；响应无凭据、Cookie、UUID、完整 URL；桌面 IPC 读取同一日志且无本地 HTTP 端口 |

## 7. 代理协议与网络泄漏

### 7.1 协议矩阵

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| PROXY-001 | HTTP/SOCKS5 认证代理 | 正确凭据成功，错误凭据返回认证失败；域名由节点解析 |
| PROXY-002 | VLESS over TCP/TLS | 受控节点可访问探针页；TLS 握手与认证正确；无直连回退 |
| PROXY-003 | VLESS REALITY/WS/gRPC | 各传输按能力清单通过；REALITY 参数错误有明确错误类 |
| PROXY-004 | VMess TCP/WS/gRPC | 受控节点可访问；AEAD 与传输组合正确；错误用户 ID 被拒绝 |
| PROXY-005 | Shadowsocks AEAD/2022 | 各声明方法与插件可连接；错误密码/方法/插件被拒绝 |
| PROXY-006 | Trojan TLS/WS | TLS 与密码认证正确；证书错误、错误密码有明确分类 |
| PROXY-007 | Hysteria2 QUIC（Tier B） | 按 Tier B 记录：受控节点可访问；容器允许 UDP 出站时成功；UDP 被阻止时明确报错；不得作为 Tier A 发布门禁或 PRD 必选协议 |
| PROXY-008 | Hysteria2/WireGuard/TUIC | Hysteria2 与 WireGuard 按 Tier B 清单记录；WireGuard 必须有隧道内 DNS，缺失配置被拒绝；TUIC 在 v1 被拒绝；均不静默降级 |

### 7.2 配置与引擎生命周期

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| PROXY-009 | 类型化配置校验 | 未知字段、错误端口、缺失凭据、非法传输组合被字段级拒绝 |
| PROXY-010 | 分享链接导入 | 五种链接合法导入；不支持参数、超大输入、畸形 base64 被拒绝；不联网、不执行 |
| PROXY-011 | 动态资源生命周期 | 启动创建出站/入站/规则；停止删除入站/规则、清空 `proxy_outbound_key`/`proxy_inbound_tag`/`proxy_rule_tag`/`proxy_port` 并按非终态引用释放出站；`ref_count` 不含终态运行时；无持久化凭据配置 |
| PROXY-012 | 引擎校验与清单 | 篡改二进制/清单拒绝启动；协议矩阵、MPL 许可、来源信息完整；未声明能力不可选 |
| PROXY-013 | 共享引擎与入站隔离 | 全宿主一个 Xray 引擎；每个运行档案独立入站/端口/规则；引擎先于内核就绪；共享引擎不随单档案停止 |
| PROXY-014 | 共享引擎故障与重建 | 运行中强杀引擎；所有运行档案网络失败；无直连/其他节点回退；产生 `proxy.engine.failed`；重启后按原端口重建并产生 `proxy.engine.rebuilt`；重建失败时先拆除内核/进程组/入站/规则/出站引用/配额锁再写 `failed`，无终态后活内核 |

### 7.3 网络与泄漏

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| PROXY-015 | DNS canary | 本机无目标域名查询；节点收到域名或完成解析；仅节点主机名可能本地 bootstrap 解析 |
| PROXY-016 | WebRTC | 默认无非代理 UDP；无直连候选；高级关闭有审计 |
| PROXY-017 | IPv6 | 双栈与 IPv6-only 目标不产生直连 IPv6；不支持 IPv6 的节点明确报错 |
| PROXY-018 | DoH 绕过 | 安全 DNS 不绕过代理；无法验证时启动被阻止或标记 |
| PROXY-019 | 失败关闭 | 引擎关闭、节点断开、错误凭据、错误 TLS/REALITY 均导致网络失败，无直连 |
| PROXY-020 | 代理检查 | 成功/失败、延迟、出口 IP 正确；认证、TLS、拒绝连接、超时返回不同稳定错误码；网络失败以 HTTP 200 的 `ProxyCheck.error_code` 返回（同时写入事件），不返回未声明的 502；检查使用独立临时出站并清理，不改变 ref_count；检查结果只作警告，不阻断启动 |
| PROXY-021 | 并发与资源 | 高并发、空闲、半关闭连接下无 FD 或内存持续增长 |

### 7.4 秘密与边界

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| PROXY-022 | 凭据脱敏 | 进程参数、日志、事件、API 响应和持久化文件均无 UUID/密码/私钥/完整分享链接 |
| PROXY-023 | 回环边界 | 非回环地址不可连接运行档案入站或 Xray 管理 API；运行时结束后端口关闭；同用户与浏览器网络栈访问回环代理的残余风险有文档，页面不能触达管理 API 或数据库 |
| PROXY-024 | 直连模式 | 不创建入站/出站；`direct=true`、`reduced_assurance=true` 与风险标记持续可见；服务端未设置 `MIEN_ALLOW_DIRECT_EGRESS=1` 时拒绝启动，允许时写审计；不声称拥有代理模式的 DNS/DoH/IPv6/UDP 防护 |
| PROXY-025 | 恶意配置/链接 | 配置注入、路径、超长字段、非法端口、未知协议被拒绝；产品不直接透传用户 JSON |
| PROXY-026 | 脱敏视图 | UI/API 只显示非秘密字段与 `credential_set`；无法读回旧凭据；可替换保存 |
| PROXY-027 | 共享出站与连接复用 | 相同节点/凭据/传输/TLS/mux 的档案只创建一个出站并复用连接；不同凭据不共享；引用归零删除 |
| PROXY-028 | 动态并发 | 并发启动/停止不同档案不产生重复出站、孤儿入站或规则；端口与标签稳定 |
| PROXY-029 | 管理 API 边界 | API 仅回环可达；同 UID 进程调用被记录为接受风险；无沙箱模式验证内核 UID 与管理 API 处于不可互相路由的边界；操作串行、幂等；重启后引用计数与资源可对账 |

### 7.5 协议引擎准入

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| ENGINE-001 | 版本与校验 | `darwin-arm64` 与 `linux-amd64` 二进制版本、SHA-256、平台匹配 |
| ENGINE-002 | 协议矩阵 | Tier A 全部通过；Tier B 按清单记录；能力清单与实际行为一致 |
| ENGINE-003 | 动态配置与就绪 | Xray API 可添加/删除入站、出站、规则；入站在超时内就绪；失败回滚 |
| ENGINE-004 | DNS 与泄漏 | 目标域名不本地解析；WebRTC/IPv6/DoH 用例通过 |
| ENGINE-005 | 失败关闭 | 共享引擎退出/配置错误/节点失败均无直连回退 |
| ENGINE-006 | 凭据与日志 | 凭据不经 argv/日志/持久化配置；管理 API 仅回环；日志无秘密 |
| ENGINE-007 | 许可合规 | MPL-2.0 许可证、来源信息、再分发合规评审完成 |
| ENGINE-008 | 稳定性与资源 | 30 分钟连续浏览无崩溃；进程退出可清理；内存与 FD 无持续增长 |
| ENGINE-009 | 动态 API 与连接复用 | Add/Remove Inbound/Outbound/Rule 幂等；共享出站连接复用；引用计数可重建 |

## 8. 控制面与安全

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| API-001 | 未认证访问 | 非健康端点返回 401 |
| API-002 | 会话与 Token | 登录成功；Token 只显示一次；撤销立即生效 |
| API-003 | Scope | 每个操作声明 scope；越权操作返回 403；低权限 Token 不能创建更高权限 Token |
| API-004 | CSRF | 跨源、缺失或错误 CSRF 的写请求被拒绝 |
| API-005 | 登录限流 | 超限返回 429，消息不泄露用户是否存在 |
| API-006 | 错误结构 | 统一 code/message/request_id，无敏感值 |
| API-007 | 分页 | 游标无重复遗漏，页大小上限生效 |
| API-008 | 幂等 | 相同键返回原结果，冲突键 409 |
| API-009 | WebSocket | 一次性票据只能使用一次；无长期 URL Token；Cookie 升级校验 Origin；断线可按 after 补齐；心跳有效 |
| API-010 | 健康与就绪 | 语义分离；纯 headless 部署不要求 Xvfb；响应无敏感配置 |
| API-011 | 请求限制 | 超大/畸形/慢请求被拒绝，内存不增长 |
| API-012 | 审计 | 关键操作有事件；`request_id`、actor、target 完整；敏感字段脱敏 |
| API-013 | 静态 UI 安全 | CSP、nosniff、Referrer-Policy、frame-ancestors 生效 |
| API-014 | 代理 API | CRUD、分享链接导入、检查可用；非法协议/参数被拒绝；响应脱敏 |
| API-015 | 桌面 IPC 契约 | 档案、代理、运行时、内核命令与 HTTP 语义一致；错误结构一致；桌面默认无本地 HTTP 管理端口 |
| API-016 | 桌面事件回放 | 刷新 WebView 后补齐遗漏事件；按标识去重；顺序正确；无敏感字段 |
| API-017 | 首次登录改密 | 初始密码登录返回 `must_change_password=true`；未改密写操作 403；改密后旧会话失效 |
| API-018 | 查看器端点 | 默认 `VIEWER_NOT_AVAILABLE`；启用后返回一次性 URL，二次使用失败 |
| API-019 | 审计筛选契约 | 审计端点与 `AuditEvent` 支持 `event_type` 与 `profile_id` 筛选，保留 `action` 兼容；非法枚举返回 422；组合筛选与游标分页无重复遗漏；UI 使用同一字段名 |
| API-020 | 日志端点边界 | `GET /api/v1/profiles/{id}/logs?limit=` 有界且脱敏，来源为运行时日志环或轮转文件；limit 上限生效，未知档案 404，未认证 401；无凭据、Cookie、UUID、完整 URL；桌面 IPC 与 HTTP 同语义 |
| EXPORT-001 | 明文导出 | 默认导出不含代理凭据、Token、Cookie；schema 可重新校验 |
| EXPORT-002 | 加密导出/导入 | 正确口令可完整恢复配置；错误或篡改口令被拒绝；密文不可读 |
| EXPORT-003 | 导入冲突 | 冲突项在确认后分配新 ID，否则返回 `PROFILE_IMPORT_CONFLICT` |
| EXPORT-004 | 导入安全 | 超大、错误 schema、路径穿越、符号链接和恶意代理被拒绝 |
| EXPORT-005 | 服务端传输 | Docker Web UI 可通过 import/export 端点操作；独立大小上限和 CSRF 生效 |
| EXPORT-006 | 档案复制 | `copy_fingerprint=true` 复制显式字段且总是生成新种子；不存在 `FINGERPRINT_SEED_CONFLICT` 路径；导出不含顶层 `window`，窗口尺寸只随指纹规格携带 |
| SEC-001 | 密钥静态保护 | 代理凭据字段为密文；桌面钥匙串/服务端文件权限正确 |
| SEC-002 | 路径穿越 | 导入、档案目录、符号链接均不能越界 |
| SEC-003 | 恶意导入 | 超大、错误 schema、非法代理、XSS 字符串被拒绝或转义 |
| SEC-004 | 容器基线 | 非 root、无多余能力、只读根、无 Docker socket；宿主侧 seccomp 文件被 Docker daemon 加载，镜像内审计副本摘要一致；镜像扫描通过 |
| SEC-005 | 内核与引擎供应链 | 校验和不匹配拒绝；发布物含许可声明；无静默下载 |
| SEC-006 | 依赖与许可 | Rust/前端依赖审计无未接受的高危项；Rust、前端与容器许可扫描全部通过 |
| SEC-007 | MPL 合规 | Xray-core 许可证、来源信息、再分发合规评审完成 |
| SEC-008 | 引擎 API 保护 | 管理 API 仅回环；凭据不进入 argv/日志/持久化配置；API 无 UUID/密码/私钥 |
| SEC-009 | 桌面 WebView 边界 | 远程内容不能获得 IPC 能力；CSP/capabilities 生效；外链交给系统浏览器；文件路径经 dialog 与校验 |

## 9. 性能、容量与浸泡

| 编号 | 指标 | 目标 |
| --- | --- | --- |
| PERF-001 | 应用冷启动 | ≤3 秒（macOS 桌面，不含内核首次解压） |
| PERF-002 | 档案启动 | p95 ≤5 秒（内核与共享协议引擎已就绪，有头） |
| PERF-003 | 档案停止 | p95 ≤3 秒（正常路径） |
| PERF-004 | 列表首屏 | 100 个档案 ≤500 毫秒 |
| PERF-005 | 并发 | 8 vCPU / 16 GB 下 10 个档案并发运行 |
| PERF-006 | 24 小时浸泡 | 无孤儿、无数据库损坏、无 FD/内存持续增长 |
| PERF-007 | 代理吞吐与复用 | 共享协议引擎转发不造成不可接受的延迟；记录各协议与连接复用基线 |
| PERF-008 | 容器启动 | 记录 Xvfb + 服务的冷启动时间；不作为硬门禁 |

## 10. 打包与发布

| 编号 | 用例 | 通过条件 |
| --- | --- | --- |
| PKG-001 | macOS AdHoc 签名 | 应用、嵌套 Chromium 内核、Xray-core 和 frameworks 通过 `codesign --verify --deep --strict`；应用与 Chromium 内核带且只带 JIT 两项 entitlements，Xray/frameworks 不带；`--options runtime` 与 `Signature=adhoc` 已断言；不要求 Developer ID |
| PKG-002 | Gatekeeper 行为 | 从浏览器下载的 DMG 保留 quarantine 后首次启动被阻止；发布说明提供 SHA-256 校验和手动放行步骤；不要求无提示打开 |
| PKG-003 | 自动更新 | Tauri 更新清单签名有效，拒绝未签名或降级包；不依赖 Apple 签名或公证 |
| PKG-004 | Docker 构建 | 多阶段构建可复现，镜像非 root，健康检查有效 |
| PKG-005 | Docker 数据卷 | 删除并重建容器后数据保留 |
| PKG-006 | SBOM 与许可 | 镜像和应用包含 SBOM、BSD-3-Clause、MPL-2.0 许可与来源信息 |
| PKG-007 | 漏洞扫描 | 无未接受的高危/严重漏洞 |
| PKG-008 | 发布说明 | 列出内核缺陷、代理协议矩阵、节点 bootstrap 泄漏、无头限制、跨系统限制、显示与字体限制 |
| PKG-009 | GitHub Actions 发布 | 手动 `release_target` 仅在 main 已绿时发布；draft reservation 绑定 version/source；产物含 SHA-256SUMS、SBOM、许可证、宿主侧 seccomp 发布物、`release-identity.json` 与覆盖 DMG/容器/SBOM/seccomp 的 attestation；不存在 Apple 证书或公证 secret |

## 11. 缺陷分级与发布门禁

| 级别 | 定义 | 发布影响 |
| --- | --- | --- |
| P0 | 凭据泄露、认证绕过、远程代码执行、容器逃逸、数据损坏 | 阻断发布，无豁免路径 |
| P1 | 指纹内部矛盾、代理/DNS/WebRTC/IPv6 泄漏、协议引擎失败后直连回退、运行时重复或孤儿、崩溃循环、导入或档案路径穿越、沙箱状态未如实披露 | 泄漏/回退/穿越/崩溃循环/孤儿类别阻断发布且不得整类豁免；其余 P1 仅可逐项具名限期接受 |
| P2 | 单站点兼容问题、已知宿主绑定字段差异、无头特征差异、非关键 UI 缺陷 | 可带已知限制发布，需记录 |
| P3 | 文案、布局、非关键性能问题 | 可排期修复 |

发布门禁：

- P0 数量必须为 0；任何书面风险接受、环境决策或人工备注都不能豁免 P0。
- P1 中的凭据/网络泄漏（DNS、WebRTC、IPv6、DoH）、失败关闭回退（直连或其他节点）、导入与档案路径穿越、崩溃循环、运行时重复或孤儿、沙箱状态未披露必须为 0；不得整类豁免或按环境放宽。
- 其余 P1 只能逐项接受，必须记录缺陷编号、具名接受人、影响范围、到期日和修复计划；未满足则阻断发布。P2/P3 可记录后排期，但不得掩盖 P0/P1。
- PRD-ACC-001 至 PRD-ACC-010 全部有证据，缺失任何一项即阻断，不得以“后续补充”推进。
- 内核准入 KERNEL-001 至 KERNEL-010 通过。
- 协议引擎准入 ENGINE-001 至 ENGINE-009 通过，Tier A 协议矩阵、共享出站与分享链接导入通过；Tier B 结果只记录，不作为门禁。
- EXPORT-001 至 EXPORT-006 通过；UI-001 至 UI-022 通过；RT-017 至 RT-025、API-019 至 API-020 通过。
- 指纹、代理泄漏、认证与容器基线测试全部通过。
- 隔离 proof（`enforced`/`unsandboxed`/`unknown`）与日志尾部端点的脱敏证据存在；停止档案的降低保证可见。
- 桌面管理 UI 默认无本地 HTTP 管理端口；IPC 与 HTTP 语义一致；事件回放与 WebView 边界测试通过。
- 已知限制在发布说明和 UI 中可见。
- 门禁自检（GATE-001）：测试报告必须按固定 required ID 集合给出机器可读结果；`KERNEL-001..010`、`ENGINE-001..009`、`PROXY-001..029`、`EXPORT-001..006`、`UI-001..022`、`RT-017..025`、`API-019..020`、`SEC-001..009`、`PRD-ACC-001..010`、`PKG-001..009` 中任一项缺失、`fail`、`error`、`skipped` 或非 `pass` 都拒绝创建草稿与 promote；P0 非零或 P1 阻断类别非零同样拒绝。

## 12. 证据与报告

每个发布候选生成一份测试报告，包含：

- 测试环境、内核版本、协议引擎版本、应用版本、容器镜像摘要。
- 指纹探针原始 JSON、哈希和三次启动对比。
- 各协议节点测试结果、引擎配置生命周期、代理 canary 日志、WebRTC 候选、IPv6 与 DoH 结论。
- 生命周期事件序列、进程与端口快照；六态转换与 `isolation` 取值的落库快照。
- 同档案并发 start/stop、跨档案数据隔离、删除/恢复/purge 的目录快照与 `deletion_error` 重试记录。
- 崩溃循环与共享引擎重建失败的事件序列，以及终态后的进程、端口、入站/规则与出站引用计数快照。
- 日志尾部端点/IPC 的响应样例与脱敏断言；活动筛选查询与对应 UI 截图。
- 可访问性证据：每个 surface 的 axe 或等价扫描报告、200% zoom、对比度、焦点顺序与 reduced motion 断言。
- 门禁自检结果：P0/P1 计数、逐项接受记录与 PRD-ACC-001 至 PRD-ACC-010 证据索引。
- 安全测试结果、扫描报告和残余风险。
- 性能数据与浸泡曲线。
- 未通过项、缺陷编号、接受人、到期日和影响范围。

报告与发布物分离存储，避免把测试产物提交到代码仓库。第三方站点截图只作为辅助证据，不作为唯一通过条件；UI proof 不得引用第三方站点结论。缺少上述任一类证据即视为报告不完整，按门禁阻断处理。
