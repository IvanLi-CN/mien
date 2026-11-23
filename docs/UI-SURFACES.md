# Mien UI surface map

本文补齐编码前需要的界面边界、路由、状态和运行模式选择器。首屏视觉基线仍以 [`../DESIGN.md`](../DESIGN.md) 和 [`design/profiles-dark.png`](design/profiles-dark.png) 为准；其他界面尚未有正式视觉稿，但字段、状态和限制已经固定。

## 1. 路由与信息架构

| 路径 | Surface | 主要动作 |
| --- | --- | --- |
| `/profiles` | Profiles 首屏 / Space 工作区 | 浏览空间、打开/停止运行档案、创建档案 |
| `/profiles/new` | 创建档案 | 选择预设、配置指纹、代理和运行模式 |
| `/profiles/:id` | 档案详情 | 查看 proof、运行时、日志、代理和限制 |
| `/profiles/:id/edit` | 编辑档案 | 修改指纹、代理、启动地址和运行偏好 |
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
2. 指纹：平台、平台版本、品牌、品牌版本、CPU 核心数、语言、时区、窗口尺寸、种子。
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

### 交互状态

- 创建/保存中：按钮 loading，禁止重复提交。
- 校验失败：定位到字段；顶部显示稳定错误码与可复制诊断信息。
- 未保存离开：二次确认。
- 代理不可用：允许保存但启动前显示警告；选择“检查失败禁止启动”时阻断启动。
- 内核能力缺失：显示具体字段和缺失能力，禁止保存或启动。

## 3. 档案详情

必须显示：

- 运行状态、模式、`reduced_assurance`、`ua_scope`。
- 四段 proof：fingerprint、proxy、runtime、isolation。
- 代理线路、节点地区、延迟、最近检查时间。
- 指纹种子、内核版本、代理 ID；不显示凭据。
- 启动/停止、编辑、复制、归档。
- 日志尾部、稳定错误码和复制诊断。
- 限制说明：宿主绑定显示/字体、无头或伪有头、跨系统画像、共享引擎故障域。

状态行为：

- `starting`：显示启动阶段和取消入口；不显示虚假“已运行”。
- `running`：显示停止；模式相关限制持续可见。
- `stopping`：显示停止进度。
- `crashed`/`failed`：显示错误码、最近日志和重试入口。
- 共享引擎故障：显示全局网络中断，所有运行档案 proof 变为失败或降级。

## 4. 代理库与编辑器

- 列表显示名称、协议、节点、端口、`credential_set`、最近检查和错误类。
- 创建/编辑使用类型化字段；只展示当前 Xray 能力清单支持的协议和传输。
- TUIC 不显示；如果导入 TUIC 链接，明确拒绝 `TUIC_NOT_SUPPORTED`。
- 分享链接导入只生成草稿，不自动保存或启动。
- 检查动作通过共享引擎临时资源执行；结果包含延迟、出口 IP（如配置）和错误类。
- 凭据编辑只显示“已设置”；不能读回旧值；替换后保存。
- 代理被运行档案引用时禁止删除，返回 `RESOURCE_IN_USE`。

## 5. 内核与引擎

- 展示已安装版本、平台、SHA-256、active、能力清单摘要和已知缺陷。
- 内核显示 `headless_chrome_ua_patch`、宿主绑定限制和已知 issue。
- 引擎显示 `connection_reuse`、协议矩阵、TUIC 不支持、MPL-2.0 与来源信息。
- 切换 active 需要确认；运行中的档案不被静默替换。
- 版本切换后必须提示连接池会重置，运行档案需要重启。

## 6. 活动与审计

- 运行事件：starting/running/stopping/stopped/crashed/failed。
- 代理事件：engine.failed、engine.rebuilt、check.completed。
- 配置事件：profile、proxy、kernel、engine、token。
- 审计事件只显示非敏感元数据；不显示 Cookie、Token、UUID、私钥或完整分享链接。
- 支持按档案、事件类型和时间筛选；空状态说明没有事件，而不是显示空白列表。

## 7. 响应式与可访问性

- 1440、1024、768、390 宽度都不得横向滚动。
- 1024 使用图标侧轨；768 使用横向 Space 切换；390 单列、操作固定底部。
- 所有主要操作可键盘完成；焦点环可见；模式选择器使用原生 radio/select 语义。
- 错误、加载、空状态和禁用原因必须可读；状态不能只依赖颜色。
- 视觉证据在每个 surface 实现后补齐；当前只有 Profiles 首屏有已批准 comp。

## 8. 与契约的关系

- API 字段见 [`contracts/openapi.yaml`](contracts/openapi.yaml)。
- 事件结构见 [`contracts/events.schema.json`](contracts/events.schema.json)。
- 错误码见 [`ERROR-CODES.md`](ERROR-CODES.md)。
- 运行模式见 [`specs/browser-runtime/SPEC.md`](specs/browser-runtime/SPEC.md)。
- Profiles 首屏方向见 [`../.impeccable/surfaces/ui-src-pages-profiles.md`](../.impeccable/surfaces/ui-src-pages-profiles.md)。
