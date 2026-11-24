# Mien 测试 fixture 目录

本文列出编码与验证前必须建立的 fixture。fixture 不提交真实代理凭据、客户数据或第三方站点截图；测试报告与二进制产物不进入代码仓库。

## 0. 根目录、所有权与供应

- Fixture 根目录固定为 `fixtures/`；路径按本文的 `kernel/`、`fingerprint/`、`profile/`、`proxy/`、`dns/`、`network/`、`xray/`、`engine/`、`runtime/`、`api/`、`events/`、`db/`、`ui/` 分类。分类目录与本文表格必须一一对应，新增分类必须同时更新本文与 `fixtures/provenance.lock.json`。
- 可提交：JSON、schema、探针页、期望输出和 README。
- 不提交：内核、Xray 二进制、容器镜像、代理凭据、浏览器用户数据和测试报告。
- 二进制和镜像由 `xtask fixtures fetch` 按 manifest SHA-256 下载或构建；缓存目录不进入 Git。
- 每个目录的 owner 在 README 中标注；变更 fixture 必须同时更新 manifest、期望输出和引用测试。

### 0.1 Provenance

- 每个 fixture 目录必须有 `PROVENANCE.json`，或引用根级 `fixtures/provenance.lock.json` 中的同名条目；`xtask fixtures verify` 在测试前校验覆盖完整、摘要一致。
- 第三方来源条目至少包含：`owner`、`origin`（`upstream`/`authored`）、`fixture_version`、`source_url`、`upstream_version` 或 `commit`、`license`、`retrieved_at`（UTC）、`fetch_command`、`asset_sha256`、`binary_sha256`（存在可执行文件时）、`size` 与 `notes`。
- 自写 JSON、schema、探针页和期望输出使用 `origin=authored`，许可字段填 `Apache-2.0`，并在 README 标明作者与用途；每个 authored 文件必须记录 `content_sha256`，每个目录或 glob fixture 必须记录排序后的 `tree_sha256`（文件相对路径 + 内容摘要），不能只用文件大小或修改时间。
- 已安装内核与引擎 fixture 同时记录发布资产 `asset_sha256` 与解包后可执行文件 `binary_sha256`；运行时校验只使用后者。
- `xtask fixtures fetch` 必须先校验 provenance 中的摘要再写入缓存；摘要不符、缺少来源或许可证不兼容时失败，不得静默使用缓存。
- fixture 不得包含真实凭据、客户数据或第三方生产站点快照；需要秘密时使用本地临时 secret，并在清理步骤中删除。

## 1. 内核、指纹与档案

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `kernel/148/<platform>/` | 148 发行包解压后的内核；provenance 同时记录 `asset_sha256` 与 `binary_sha256`，manifest 只记录发布资产 `sha256` | 启动、双层校验和、版本绑定 |
| `kernel/148/<platform>/manifest.json` | 使用 [`contracts/manifests/`](contracts/manifests/) 的候选清单 | 能力清单与 UI 校验 |
| `kernel/manifest-cases/` | `sandbox_kind`、`network_controls`（`disabled`/`unsupported`）、`flagSpec.per_platform_values`、`window-size` 的合法与缺能力清单 | 清单 schema、能力门禁与降级校验 |
| `fingerprint/presets/*.json` | 主机对齐、跨系统画像、最小/最大显式字段 | 规格校验 |
| `fingerprint/golden-argv/*.json` | 规格 → 确定性 argv | 纯函数 golden 测试 |
| `fingerprint/probe/` | 本地探针页；采集 UA、UA-CH、canvas、audio、WebGL、字体、WebRTC | 稳定性与差异测试 |
| `fingerprint/headless-ua/` | 有头、无头、伪有头的探针快照 | 验证伪有头仅改 UA |
| `profile/export/` | 明文、加密、错误口令、篡改容器、冲突导入样例 | 导入导出与凭据移除 |
| `profile/import/` | dry-run 校验、`new_id`/`abort` 冲突（同一 `source_profile_id` 或同名分组/标签）、加密 passphrase、部分失败、同 seed 导入不报冲突 | 导入事务与错误报告 |
| `profile/copy/` | `copy_fingerprint=false/true`、归档档案复制、同 seed 源档案 | 复制语义与 seed 总是重新生成 |
| `profile/lifecycle/` | archive/restore、软删除到 `trash/`、trash 缺失时 restore 返回 `PROFILE_RESTORE_UNAVAILABLE`、purge 失败写 `deletion_error`、`deleted` 拒绝启动、`profiles/<id>/downloads` 的 0700 与路径包含 | 生命周期、下载目录与清理 |

## 2. 代理节点

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `proxy/http-auth/` | 受控 HTTP 代理，正确/错误用户名密码 | HTTP 认证与失败关闭 |
| `proxy/socks5-auth/` | 受控 SOCKS5，支持域名地址 | SOCKS5 与远程 DNS |
| `proxy/vless/` | TCP/TLS、REALITY、WS、gRPC 变体 | VLESS 矩阵 |
| `proxy/vmess/` | TCP/WS/gRPC、AEAD 变体 | VMess 矩阵 |
| `proxy/shadowsocks/` | AEAD 与 2022 方法 | Shadowsocks 矩阵 |
| `proxy/trojan/` | TLS、WS 变体 | Trojan 矩阵 |
| `proxy/hysteria2/` | QUIC、TLS、认证；允许/阻止 UDP 出站 | Hysteria2 与容器 UDP |
| `proxy/tuic/` | 仅用于负向测试 | v1 必须拒绝 TUIC |
| `proxy/links/` | 五种合法分享链接、错误 base64、超大和非法参数 | 分享链接导入 |
| `proxy/wireguard/` | WireGuard peer、隧道内 DNS 配置，以及缺失 DNS 的负向配置 | Tier B 与隧道内 DNS 强制 |
| `proxy/invalid/` | 错误凭据、错误 TLS/REALITY、超时、不可达 | 错误分类与失败关闭 |
| `proxy/echo/` | 返回出口 IP 与地区标记 | 代理检查 |

所有节点使用测试凭据，只存在于测试环境 secret 或本地临时文件；不得提交到 Git。

## 3. 网络泄漏

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `dns/canary/` | 本地权威 DNS 与查询日志 | 目标域名不得本地解析 |
| `network/ipv6/` | IPv6-only 与双栈目标 | IPv6 直连阻断 |
| `network/webrtc/` | 采集 ICE 候选的本地页 | 非代理 UDP/直连候选 |
| `network/doh/` | 模拟安全 DNS 的站点 | DoH 绕过检测 |
| `network/echo/` | 出口 IP/地区回显 | 代理路径与地区一致性 |
| `dns/blocked/` | 递归解析被阻断与超时响应 | 远程 DNS 失败分类（`PROXY_DNS_FAILED`） |

## 4. Xray 共享引擎

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `xray/bootstrap.json` | API 入站、block 出站、API 路由 | 引擎启动 |
| `xray/outbound-golden/*.json` | 代理配置 → 出站配置 | 协议映射 |
| `xray/inbound-golden/*.json` | runtime id + port → HTTP 入站 | 入站与标签 |
| `xray/route-golden/*.json` | runtime → 路由规则 | 默认 block 与规则顺序 |
| `xray/outbound-key-vectors/*.json` | 规范对象、凭据指纹、期望 SHA-256 | `outbound_key` 稳定性 |
| `xray/refcount/` | 三档案共享同一出站、不同凭据不共享；终态运行时不计入，停止/失败 teardown 在同一事务递减并清空 `proxy_*`，只递减一次；归零出站删除失败可作为孤儿重试 | 引用计数与连接复用 |
| `xray/crash/` | 强杀引擎、原端口重建、重建失败 | 故障关闭与恢复 |
| `xray/api-boundary/` | 非回环 API/入站探测 | API 与入站只绑定回环 |
| `xray/check/` | 检查失败时 `ProxyCheck` 的 200 响应与事件；拒绝连接、认证、TLS、超时、DNS | 代理检查错误映射 |
| `engine/xray-core/` | 锁定的 Xray 发布资产、解包二进制、`asset_sha256`/`binary_sha256` 与 manifest | 引擎安装、校验与历史版本 |

## 5. 运行模式

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `runtime/headful/` | macOS 宿主 / Docker Xvfb 有头启动 | 基线 |
| `runtime/headless/` | `--headless=new`，记录 UA 与 Headless 特征 | 降低保证 |
| `runtime/pseudo-headful/` | 声明与未声明 `headless_chrome_ua_patch` | 能力门禁与 UA 范围 |
| `runtime/xvfb/` | `MIEN_SCREEN`、openbox、窗口尺寸 | 显示行为 |
| `runtime/novnc/` | 一次性令牌、过期、故障 | 查看器边界 |
| `runtime/sandbox/` | 严格沙箱、服务端无沙箱显式授权、不可用、桌面拒绝；无沙箱时不可读 `/data/secrets`、不可访问数据库与 Xray API，子进程 `env_clear` 白名单 | 安全门禁与 blast radius |
| `runtime/isolation/` | `enforced`/`unsandboxed`/`unknown` 的 API 与 UI 状态 | `isolation` 持久化与 proof strip |
| `runtime/logs/` | 日志环、轮转文件、超限与脱敏金丝雀 | 有界日志尾部端点与脱敏 |

## 6. 控制面与事件

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `api/openapi/` | 由 OpenAPI 生成的客户端与请求样例 | 契约测试 |
| `api/errors/` | 每个稳定错误码的响应样例；`ProxyCheck` 错误的 200 `ok=false` 样例；`DIRECT_EGRESS_DISABLED`、`PROFILE_RESTORE_UNAVAILABLE`、`PROFILE_NOT_DELETED`、`EVENT_REPLAY_WINDOW_EXPIRED`、版本未找到与分组/标签未找到样例 | 错误目录与 `ErrorCode` 枚举校验 |
| `api/auth/` | 会话、Bearer、scope、CSRF、Origin、限流、管理员引导与首次改密后删除引导秘密 | 认证授权 |
| `api/audit/` | `event_type`/`profile_id`/`action` 筛选与分页 | 活动记录契约 |
| `events/*.json` | 每个事件类型的合法/非法实例 | 事件 schema |
| `events/replay/` | 整数 `after=sequence` 游标、空洞、重启后续播 | 事件回放与单调序列 |
| `db/migrations/` | 空库；含 `admin_credentials`/`deleted_at`/`deletion_error`/`isolation` 的旧 schema；`profiles.window_json` 缺失/非法且 `fingerprint_json.window` 也缺失的 active/archived/deleted 行；字符串 `event_log` 游标表；`kernel_installs`/`proxy_engine_installs` 中 `binary_sha256` 为 NULL 的迁移记录；迁移中断；恢复 | 迁移、preflight 阻断与完整性 |
| `db/restore/` | 备份、恢复、主密钥缺失 | 数据恢复 |
| `ui/` | viewport、locale、视觉基线、axe 配置 | UI、可访问性与响应式 |

## 7. 证据与命名

- 每个 fixture 必须有 `README.md`，写明启动方式、依赖、测试数据来源和清理方式。
- 测试结果按 `test-report/<version>/<platform>/<suite>/` 保存；不提交到源码。
- fixture 不能依赖真实客户、真实代理服务或第三方生产站点；外部站点只作为辅助观察。
- 任何 fixture 含秘密时必须使用本地临时 secret，并在测试结束后删除。
- 发布前把 fixture 版本、provenance 摘要（含 `asset_sha256`/`binary_sha256`/`content_sha256`/`tree_sha256`）、内核版本、引擎版本和镜像摘要写入测试报告；报告能追溯到具体 fixture 内容哈希。
- 测试报告必须给出每个 suite 使用的 fixture 目录与 `provenance.lock.json` 哈希；引用未登记 fixture 或摘要不符的测试结果无效。
- 第三方二进制、镜像和探针页必须记录许可证与来源；许可证不兼容或缺失来源时不得进入 fixture 缓存，也不得出现在发布测试报告中。

## 8. 参考

- [`TEST-PLAN.md`](TEST-PLAN.md)
- [`contracts/openapi.yaml`](contracts/openapi.yaml)
- [`contracts/events.schema.json`](contracts/events.schema.json)
- [`XRAY-MAPPING.md`](XRAY-MAPPING.md)
- [`SECURITY.md`](SECURITY.md)
- [`ERROR-CODES.md`](ERROR-CODES.md)
