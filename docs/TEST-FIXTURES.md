# Mien 测试 fixture 目录

本文列出编码与验证前必须建立的 fixture。fixture 不提交真实代理凭据、客户数据或第三方站点截图；测试报告与二进制产物不进入代码仓库。

## 1. 内核与指纹

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `kernel/148/<platform>/` | 148 发行包解压后的内核，SHA-256 记录在 manifest | 启动、校验和、版本绑定 |
| `kernel/148/<platform>/manifest.json` | 使用 [`contracts/manifests/`](contracts/manifests/) 的候选清单 | 能力清单与 UI 校验 |
| `fingerprint/presets/*.json` | 主机对齐、跨系统画像、最小/最大显式字段 | 规格校验 |
| `fingerprint/golden-argv/*.json` | 规格 → 确定性 argv | 纯函数 golden 测试 |
| `fingerprint/probe/` | 本地探针页；采集 UA、UA-CH、canvas、audio、WebGL、字体、WebRTC | 稳定性与差异测试 |
| `fingerprint/headless-ua/` | 有头、无头、伪有头的探针快照 | 验证伪有头仅改 UA |

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

## 4. Xray 共享引擎

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `xray/bootstrap.json` | API 入站、block 出站、API 路由 | 引擎启动 |
| `xray/outbound-golden/*.json` | 代理配置 → 出站配置 | 协议映射 |
| `xray/inbound-golden/*.json` | runtime id + port → HTTP 入站 | 入站与标签 |
| `xray/route-golden/*.json` | runtime → 路由规则 | 默认 block 与规则顺序 |
| `xray/outbound-key-vectors/*.json` | 规范对象、凭据指纹、期望 SHA-256 | `outbound_key` 稳定性 |
| `xray/refcount/` | 三档案共享同一出站、不同凭据不共享 | 引用计数与连接复用 |
| `xray/crash/` | 强杀引擎、原端口重建、重建失败 | 故障关闭与恢复 |
| `xray/api-boundary/` | 非回环 API/入站探测 | API 与入站只绑定回环 |

## 5. 运行模式

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `runtime/headful/` | macOS 宿主 / Docker Xvfb 有头启动 | 基线 |
| `runtime/headless/` | `--headless=new`，记录 UA 与 Headless 特征 | 降低保证 |
| `runtime/pseudo-headful/` | 声明与未声明 `headless_chrome_ua_patch` | 能力门禁与 UA 范围 |
| `runtime/xvfb/` | `MIEN_SCREEN`、openbox、窗口尺寸 | 显示行为 |
| `runtime/novnc/` | 一次性令牌、过期、故障 | 查看器边界 |
| `runtime/sandbox/` | 严格沙箱、无沙箱显式授权、不可用 | 安全门禁 |

## 6. 控制面与事件

| Fixture | 内容 | 用途 |
| --- | --- | --- |
| `api/openapi/` | 由 OpenAPI 生成的客户端与请求样例 | 契约测试 |
| `api/errors/` | 每个稳定错误码的响应样例 | 错误目录 |
| `api/auth/` | 会话、Bearer、scope、CSRF、限流 | 认证授权 |
| `events/*.json` | 每个事件类型的合法/非法实例 | 事件 schema 与回放 |
| `db/migrations/` | 空库、旧 schema、迁移中断、恢复 | 迁移与完整性 |
| `db/restore/` | 备份、恢复、主密钥缺失 | 数据恢复 |

## 7. 证据与命名

- 每个 fixture 必须有 `README.md`，写明启动方式、依赖、测试数据来源和清理方式。
- 测试结果按 `test-report/<version>/<platform>/<suite>/` 保存；不提交到源码。
- fixture 不能依赖真实客户、真实代理服务或第三方生产站点；外部站点只作为辅助观察。
- 任何 fixture 含秘密时必须使用本地临时 secret，并在测试结束后删除。
- 发布前把 fixture 版本、内核版本、引擎版本和镜像摘要写入测试报告。

## 8. 参考

- [`TEST-PLAN.md`](TEST-PLAN.md)
- [`contracts/openapi.yaml`](contracts/openapi.yaml)
- [`contracts/events.schema.json`](contracts/events.schema.json)
- [`XRAY-MAPPING.md`](XRAY-MAPPING.md)
