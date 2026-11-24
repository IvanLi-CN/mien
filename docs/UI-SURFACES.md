# Mien UI surface map

本文补齐编码前需要的界面边界、路由、状态和运行模式选择器。首屏视觉基线仍以 [`../DESIGN.md`](../DESIGN.md) 和 [`design/profiles-dark.png`](design/profiles-dark.png) 为准；其他界面尚未有正式视觉稿，但字段、状态和限制已经固定。

## 1. 路由与信息架构

| 路径 | Surface | 主要动作 |
| --- | --- | --- |
| `/profiles` | Profiles 首屏 / Space 工作区 | 浏览空间、打开/停止运行档案、创建档案、导入档案 |
| `/profiles/new` | 创建档案 | 选择预设、配置指纹、代理和运行模式 |
| `/profiles/:id` | 档案详情 | 查看 proof、运行时、日志、代理和限制 |
| `/profiles/:id/edit` | 编辑档案 | 修改指纹、代理、启动地址和运行偏好；导出档案 |
| `/proxies` | 代理库 | 创建、编辑、检查、导入分享链接 |
| `/kernels` | 内核与引擎 | 查看已安装版本、能力清单、已知缺陷、切换 active |
| `/activity` | 活动与审计 | 查看运行事件、代理检查、登录和配置变更 |
| `/settings` | 服务端设置 | Token、查看器、部署限制；桌面端只保留本地设置 |

- 桌面 Tauri 与 Docker Web UI 共用路由和组件；传输层由 `ControlTransport` 选择 Tauri IPC 或 HTTP/WS。
- 档案详情使用浮层 drawer 或独立路由；不得引入常驻右侧检查器。
- Profiles 首屏的 Space、Live session 和 profile 字段结构不变。

## 2. Profiles 创建与编辑

### 区块

1. 基本身份：名称、Space/分组、标签、备注。
2. 指纹：平台、平台版本、品牌、品牌版本、CPU 核心数、语言、时区、窗口尺寸、只读种子。窗口尺寸只属于指纹规格，不作为档案顶层字段；插件面、字体与屏幕字段为 host-bound，只显示当前宿主值。
3. 代理：选择已有代理或新建；脱敏显示节点与 `credential_set`。
4. 运行：启动地址、运行模式、CDP 自动化开关。
5. 风险：跨系统画像、无头/伪有头、无沙箱、直连代理等限制。

### 运行模式选择器

选项固定为三档：

| 值 | 标签 | 说明 |
| --- | --- | --- |
| `headful` | **有头** | 正常窗口；macOS 宿主桌面，Docker 使用 Xvfb。 |
| `headless` | **无头** | Chromium headless；不承诺 UA 结果；降低保证。 |
| `pseudo_headful` | **伪有头** | 无头运行；仅把 `HeadlessChrome` UA 改为 `Chrome`；其他 Headless 特征保留。 |

规则：

- 默认 `headful`。
- `headless` 与 `pseudo_headful` 持续显示“降低保证”标签。
- `pseudo_headful` 必须显示“仅 UA 伪装，不是有头等价”；不得使用“有头模拟”“完全有头”等词。
- 当前内核能力清单没有 `headless_chrome_ua_patch=true` 时，选项禁用并显示原因；不能保存后静默降级。
- 选择 `pseudo_headful` 时，UI 与 API 返回 `mode=pseudo_headful`、`reduced_assurance=true`、`ua_scope=chrome`。
- 运行中的档案不允许无确认切换模式；必须先停止。
- 表单失败显示字段级错误；不得把代理凭据回显到表单。
- 导入在 validate 之后确认与完成界面都提示“导入保留 seed；相同内核/规格下指纹相同”，冲突按 source_profile_id 或同名组/标签识别；不按 seed 查询或拒绝合法导入。导出明确 plain/encrypted 与 write-only 口令。

### 交互状态

- 创建/保存中：按钮 loading，禁止重复提交。
- 校验失败：定位到字段；顶部显示稳定错误码与可复制诊断信息。
- 未保存离开：二次确认。
- 代理不可用：允许保存，启动前显示警告与该代理的 `ProxyCheck.error_code`；检查失败只影响 proof 与警告，不提供“检查失败禁止启动”开关，也不阻断启动。
- 重复提交：同档案 start 只复用进行中的运行时并显示 `starting`，不得再次拉起内核；`starting` 中的 stop 作为取消，先进入 `stopping`，清理完成后显示 `stopped`。
- 内核能力缺失：显示具体字段和缺失能力，禁止保存或启动。

## 3. 档案详情

必须显示：

- 运行状态（六态）、模式、`direct`、`reduced_assurance`、`ua_scope`。
- 四段 proof：fingerprint、proxy、runtime、isolation。IS 取值为 `enforced`/`unsandboxed`/`unknown`，Profile payload/proof 只读 `runtimes.last_known_isolation`；Runtime/event 显示本次尝试的 `runtimes.isolation`，失败尝试不覆盖历史结论。
- `isolation` 呈现：`enforced` 显示“沙箱已生效”；`unsandboxed` 持续显示“无沙箱”与降低保证；`unknown` 用虚线描边加 `?` 与文本“未确认”；`stopped` 只表示“未运行”，不得覆盖 `unsandboxed` 或 `unknown` 警告，也不得与 `unknown` 共用同一呈现。
- 停止的档案仍显示最近一次运行的 `unsandboxed` 或 `unknown` 与降低保证；从未启动且无运行记录时显示 `unknown`，不得因此省略 proof。成功启动的运行时不会显示 `unknown`。
- 代理线路、节点地区、延迟、最近检查时间、`ProxyCheck.error_code`。
- 指纹种子、内核版本、代理 ID；不显示凭据。
- 启动/停止、编辑、复制、归档/恢复、删除到回收站/恢复/彻底删除。
- 日志尾部、稳定错误码和复制诊断。
- 限制说明：宿主绑定显示/字体/插件面、无头或伪有头、跨系统画像、共享引擎故障域。proof 不引用第三方指纹站点结果作为判定。
- 直连显示“直连/降低保证”，不创建代理资源、不提供代理级远程 DNS/失败关闭；内核支持时仍应用 DoH/IPv6/非代理 UDP 标志，无法应用就拒绝启动。

状态行为：

- `starting`：显示状态及有界脱敏日志尾部，不伪造未由契约提供的阶段/步骤字段；取消进入 `stopping`，仅清理完成后显示 `stopped`。
- `running`：显示停止；模式相关限制与 `isolation` 持续可见。
- `stopping`：显示停止进度；完成后清空入站/规则/端口并显示 `stopped`。
- `crashed`：内核提前退出（`starting` 或 `running` 遇异常退出）；先清理内核、入站/规则与出站引用，清理完成后显示 `crashed`、退出码、错误码、最近日志与重试入口；启用自动重启时最多 3 次递增退避，稳定后重置，超过上限停止重试并提示崩溃循环。
- `failed`：pre-resource 失败直接显示 `failed`；post-resource 启动失败、引擎失败等非崩溃类失败只有在拆除内核、入站/规则与出站引用后才进入该终态。若 teardown 尚未完成，界面保持 `stopping` 并显示重试/诊断入口，不得同时显示“运行中”或伪造终态。
- `stopped` 与 `deleted`：`archived` 与 `deleted` 均不能启动且显示原因，默认列表排除两者；回收站只包含 deleted。运行中删除返回 RUNTIME_CONFLICT；restore 缺目录显示 PROFILE_RESTORE_UNAVAILABLE；purge 失败显示 deletion_error 与重试。
- 共享引擎故障：显示代理档案全局网络中断；保留活且可归属的内核状态，重建成功恢复原端口并显示 rebuilt；直连不受影响。失败耗尽 teardown 完成才显示 failed；未知退出归属时显示 stopping 与端口隔离。
- 日志尾部：读取 `GET /api/v1/profiles/{id}/logs?limit=` 或桌面等价 IPC，显示有界脱敏内容、`limit` 与截断标记；不可用时显示稳定错误码与重试，不留空白。

## 4. 代理库与编辑器

- 列表显示名称、协议、节点、端口、`credential_set`、最近检查和错误类。
- 创建/编辑使用类型化字段；只展示当前 Xray 能力清单支持的协议和传输。
- TUIC 不显示；如果导入 TUIC 链接，明确拒绝 `TUIC_NOT_SUPPORTED`。
- 分享链接导入只生成草稿，不自动保存或启动。
- 检查动作通过共享引擎临时资源执行；结果包含延迟、出口 IP（如配置）和错误类。网络失败以 HTTP 200 的 `ProxyCheck.error_code` 呈现，同时写入事件；检查结果只作警告，不阻断保存或启动。
- 凭据编辑只显示“已设置”；不能读回旧值；替换后保存。
- 代理被运行档案引用时禁止删除，返回 `RESOURCE_IN_USE`。

## 5. 内核与引擎

- 展示已安装版本、平台、SHA-256、active、能力清单摘要和已知缺陷。
- 内核显示 `headless_chrome_ua_patch`、宿主绑定限制和已知 issue。
- 引擎显示 `connection_reuse`、协议矩阵、TUIC 不支持、MPL-2.0 与来源信息。
- 切换 active 需要确认；运行中的档案不被静默替换。
- 版本切换后必须提示连接池会重置，运行档案需要重启。

## 6. 活动与审计

- 运行事件：`runtime.starting`、`runtime.running`、`runtime.stopping`、`runtime.stopped`、`runtime.crashed`、`runtime.failed`。
- 代理事件：`proxy.engine.failed`、`proxy.engine.rebuilt`、`proxy.check.completed`。
- 配置事件：`profile.*`、`kernel.*`、`engine.*`、`token.*`；代理配置变更只通过审计事件展示，不伪造不存在的 `proxy.created/updated` 事件。
- 审计事件只显示非敏感元数据；不显示 Cookie、Token、UUID、私钥或完整分享链接。
- 支持按 `event_type`、`profile_id`、`action` 和 `from`/`to` 时间筛选，字段名与审计契约一致（`action` 为兼容字段）；组合筛选与游标分页无重复遗漏；空状态说明没有事件，而不是显示空白列表。
- 日志尾部入口按有界端点 `GET /api/v1/profiles/{id}/logs?limit=` 读取，不在活动页直接读本地文件。

## 7. 响应式与可访问性

- 1440、1024、768、390 宽度都不得横向滚动。
- 1024 使用图标侧轨；768 使用横向 Space 切换；390 单列、操作固定底部。
- 所有主要操作可键盘完成；焦点环可见；模式选择器使用原生 radio/select 语义。
- 错误、加载、空状态和禁用原因必须可读；状态不能只依赖颜色。
- 每个 surface 必须执行 axe 或等价扫描并归档报告，覆盖对比度阈值、角色/名称、200% zoom、焦点顺序、禁用原因和 `prefers-reduced-motion`；缺少扫描报告或断言失败即视为该 surface 未完成。
- 视觉证据在每个 surface 实现后补齐；当前只有 Profiles 首屏有已批准 comp。可访问性证据与视觉证据分开归档，均不提交到代码仓库。

## 8. 编辑、设置与列表状态

`/profiles/:id/edit`：

- 加载态使用骨架；保存中禁用重复提交并保留已填值。
- 字段错误定位到字段；未保存离开二次确认。
- 凭据只显示“已设置”，不读回旧值；替换后保存。
- 指纹种子不可编辑；平台、语言或时区变更时提示降低保证与需要重启。
- 代理检查结果只显示警告与 `ProxyCheck.error_code`；界面不提供“检查失败禁止启动”控件。
- 导入/导出入口与口令字段：口令为 write-only，明文与加密互斥，冲突策略可选；导出不包含顶层 `window`。
- 生命周期动作：归档/恢复、删除到回收站、恢复、彻底删除；失败显示 `deletion_error` 与重试入口。
- 保存后若档案正在运行，提示重启后生效，不静默替换运行中的配置。

`/settings`：

- 加载、空、错误与重试；提交后给出反馈与稳定错误码。
- Token 只显示一次，列表显示前缀与 scope，撤销立即生效。
- 查看器开关默认关闭；未启用时显示 `VIEWER_NOT_AVAILABLE` 与启用入口。
- 服务端模式显示部署限制（只读根、`/data` 唯一持久卷、代理与证书来源）；桌面模式隐藏服务端专属项并注明无本地 HTTP 管理端口。
- 首次登录改密：显示 `must_change_password=true`，未改密时禁用写操作并说明原因。

列表状态（`/profiles`、`/proxies`、`/kernels`、`/activity`）：

- 首屏加载使用骨架，不用空白表格。
- 空结果区分“尚无数据”和“筛选无结果”，并提供清除筛选动作。
- 加载失败显示稳定错误码、`request_id` 与重试入口。
- 分页或滚动加载保留已加载项，不闪空；重复项按标识去重。
- `archived` 与 `deleted` 默认不在 active 列表；显式筛选显示。两者均不可启动并显示原因，deleted 另提供回收站恢复入口。

## 9. 与契约的关系

- API 字段见 [`contracts/openapi.yaml`](contracts/openapi.yaml)。
- 事件结构见 [`contracts/events.schema.json`](contracts/events.schema.json)。
- 错误码见 [`ERROR-CODES.md`](ERROR-CODES.md)。
- 运行模式见 [`specs/browser-runtime/SPEC.md`](specs/browser-runtime/SPEC.md)。
- 运行状态机、`isolation` 与日志端点见 [`specs/browser-runtime/SPEC.md`](specs/browser-runtime/SPEC.md)，日志端点与审计筛选字段同时在 OpenAPI 中声明。
- 档案生命周期（active/archived/deleted）、回收站目录与 `deletion_error` 见 [`specs/profile-management/SPEC.md`](specs/profile-management/SPEC.md) 与 [`DATABASE-SCHEMA.md`](DATABASE-SCHEMA.md)。
- Profiles 首屏方向见 [`../.impeccable/surfaces/ui-src-pages-profiles.md`](../.impeccable/surfaces/ui-src-pages-profiles.md)。
