# Xray 共享协议引擎映射

本文把 `docs/specs/proxy-gateway/SPEC.md` 的代理契约落到 Xray-core v26.11.9 的进程、API、入站、出站、路由、连接复用和故障恢复。它是实现前的确定性映射；标为“准入确认”的项目必须在 M0 Xray spike 中验证后才能冻结。

## 1. 进程与版本

- 每个宿主一个 Xray-core 进程，版本固定为 `v26.11.9` 或经准入的后继补丁版本。
- 清单中的 `sha256` 是发布资产摘要；安装记录同时保存 `asset_sha256`（下载资产）与 `binary_sha256`（解包后可执行文件）。安装时校验 `asset_sha256`，启动时只校验 `binary_sha256`；文件缺失返回 `PROXY_ENGINE_NOT_FOUND`，摘要不符返回 `PROXY_ENGINE_CHECKSUM_MISMATCH`。不修改二进制。
- 管理 API 只监听 `127.0.0.1` 的随机端口；不映射到容器外或局域网。
- 代理凭据只通过回环管理 API 或受保护引导通道传递，不进入 argv、日志、事件、持久化配置或浏览器。
- 代理模式下浏览器只连接本运行档案的回环 HTTP 入站；直连模式没有入站，浏览器直接出口并标记降低保证。

## 2. 引导配置

启动时使用最小配置，只包含 API、API 入站、block 出站和 API 路由。代理出站、运行档案入站和路由规则随后通过 API 动态创建。

```json
{
  "log": {"loglevel": "warning"},
  "api": {
    "tag": "api",
    "services": ["HandlerService", "RoutingService"]
  },
  "inbounds": [
    {
      "tag": "api",
      "listen": "127.0.0.1",
      "port": 0,
      "protocol": "dokodemo-door",
      "settings": {"address": "127.0.0.1"}
    }
  ],
  "outbounds": [
    {"tag": "block", "protocol": "blackhole"}
  ],
  "routing": {
    "domainStrategy": "AsIs",
    "rules": [
      {"type": "field", "inboundTag": ["api"], "outboundTag": "api"}
    ]
  }
}
```

实现时由代理网关分配实际 API 端口并写入引导配置；`port: 0` 只是合同示意，不能让 Xray 自行分配后无法被网关发现。

失败关闭的三个不变量：

- `block`（blackhole）必须是出站列表的第一项；网关在启动后、接受任何运行档案前断言该顺序，断言失败则拒绝服务，不得补建 direct 出站或回退直连。
- Xray 路由配置没有 `routing.final` 字段，引导配置不得生成该字段；未匹配流量只能由首出站 blackhole 或显式规则处理。
- 引导配置不得包含 `routing.script`、`routing.balancers` 或其他可整体接管路由的字段；网关同时断言不存在这些字段。

## 3. 每运行档案入站

每个运行档案一个 HTTP 入站，使用预先分配的稳定端口；重启后必须在同一端口重建。

```json
{
  "tag": "rt:<runtime_id>:in",
  "listen": "127.0.0.1",
  "port": 41000,
  "protocol": "http",
  "settings": {},
  "sniffing": {"enabled": false}
}
```

Chromium 参数：

```text
--proxy-server=http://127.0.0.1:41000
--headless=new        # 仅 headless / pseudo_headful
```

- 不启用混合 SOCKS 入口；浏览器侧只使用 HTTP CONNECT。
- 入站标签由运行时标识确定；不允许复用另一个档案的标签或端口。
- 端口释放后才能分配给新运行档案；分配由代理网关串行化。

## 4. 共享出站与 outbound_key

### 4.1 规范化

`outbound_key` 是以下规范对象的 SHA-256：

```json
{
  "protocol": "vless",
  "server": "node.example.com",
  "port": 443,
  "credential_fingerprint": "hmac-sha256:<hex>",
  "transport": "ws",
  "security": "reality",
  "tls_fingerprint": "sha256:<hex>",
  "mux": {"enabled": true, "concurrency": 8}
}
```

规则：

- JSON 使用 JCS/键排序；UTF-8 编码；不在对象中放明文凭据。
- `credential_fingerprint` 使用主密钥对规范化凭据做 HMAC-SHA-256；主密钥轮换会使 key 改变，必须配合运行时迁移。
- `tls_fingerprint` 覆盖 TLS/REALITY 参数、SNI、证书校验和公钥；不同 TLS 配置不得共享。
- `mux` 必须参与 key；相同节点但 mux 设置不同属于不同出站。
- key 对外表示为 `out:<64 hex>`；数据库只存 key 和非秘密 route key，不存明文凭据。
- 相同 key 的运行档案共享出站和连接池；不同 key 即使节点相同也不共享。

### 4.2 引用计数

- 第一个使用 key 的运行档案：`AddOutbound`，`ref_count=1`。
- 后续使用相同 key：复用出站，`ref_count++`。
- 引用计数只统计非终态运行时（`starting`/`running`/`stopping`），终态 `stopped`/`crashed`/`failed` 不计数。
- 运行档案停止或失败 teardown：先删除其路由规则和入站，再在同一事务中按 `proxy_outbound_key` 执行 `ref_count--` 并清空 `proxy_outbound_key`、`proxy_inbound_tag`、`proxy_rule_tag`、`proxy_port`；提交后再删除归零的 Xray 出站。teardown 必须幂等，同一引用只释放一次；若事务后 Xray 删除失败，`proxy_outbounds` 以 `ref_count=0` 保留为可重试孤儿，不从已清空的运行时重复递减。
- 引用归零且 `RemoveOutbound` 成功：删除 `proxy_outbounds` 行；失败时保留 `ref_count=0` 的孤儿行，由对账重试，不从运行时重复递减。
- API 调用必须串行化；启动、失败回滚和引擎重建都要按非终态运行时对账引用计数；不一致时返回 `PROXY_OUTBOUND_REFCOUNT_MISMATCH` 并修复。

## 5. 协议映射

下表是首版必须实现的映射。字段名以 Xray v26.11.9 的实际 JSON schema 为准；标为“准入确认”的组合必须在 spike 中直接用 Xray 配置验证，sing-box 不参与。

| Mien 协议 | Tier | Xray protocol | 关键 settings / stream | 备注 |
| --- | --- | --- | --- | --- |
| HTTP / HTTPS | basic | `http` | server、port、用户名密码；TLS 按配置 | 认证只在出站；不参与 A/B 门禁 |
| SOCKS5 | basic | `socks` | server、port、用户名密码 | 域名地址优先；不参与 A/B 门禁 |
| VLESS | A | `vless` | `vnext` 用户 ID、flow；network、TLS/REALITY、WS/gRPC | 不把 UUID 写入浏览器 |
| VMess | A | `vmess` | `vnext` 用户 ID、security；network、TLS、WS/gRPC | AEAD |
| Shadowsocks | A | `shadowsocks` | server、port、method、password | 2022 方法按清单 |
| Trojan | A | `trojan` | server、port、password；TLS、WS | TLS 必须 |
| Hysteria2 | B | `hysteria` | `version: 2`、address、port；hysteria transport、auth、TLS | 需要 UDP 出站 |
| WireGuard | B | `wireguard` | peer 公钥、私钥、endpoint、allowed IPs；隧道内 DNS | 隧道内 DNS 必填；缺失时拒绝配置并返回 `PROXY_CONFIG_INVALID`，不得用本地 DNS 或 freedom 出站回退 |

共享 constraints：

- 所有出站默认启用 TCP 连接复用/多路复用（协议支持时）；Hysteria2 使用其原生 QUIC 连接复用，不假装可被 Xray mux 覆盖。
- 出站只允许一个目标节点；不支持多节点 selector 或自动故障切换。
- 不生成 `freedom`/direct 出站给浏览器流量；`block` 是首出站。
- 不启用 Clash API、远程控制或订阅更新。

## 6. 路由与 DNS

- 每个运行档案一条路由规则：`ruleTag: "rt:<id>:rule"`，`inboundTag: ["rt:<id>:in"]` → `outboundTag: <outbound_key>`；出站 tag 与 `proxy_outbounds.outbound_key` 完全相同，不重复添加 `out:` 前缀。
- `AddRule` 必须使用 `shouldAppend=true` 增量添加，不得清空或重排已有规则；默认的清空-追加语义不允许用于运行时路由。
- 删除规则只能按 `ruleTag` 调用 `RemoveRule`，不得按位置或重建整个规则表；重复删除同一 tag 必须幂等。
- Xray 路由配置没有 `routing.final` 字段，网关不得生成；`block` 必须是出站列表第一项，网关在启动时断言该不变量，断言失败则拒绝服务。未匹配、引擎故障或规则缺失时必须断网，不回退直连。
- 目标域名由 Xray 出站或代理内 DNS 解析；只有代理节点主机名允许本地 bootstrap 解析。
- WireGuard 必须声明隧道内 DNS；无法提供时拒绝该配置，不存在 `LIMITATION_LOCAL_DNS` 的可用路径，也不得为该配置生成 freedom 或本地 DNS 回退。
- DoH 由 `network.doh_mode=disabled` 和内核能力清单声明的启动开关关闭；IPv6 由 `network.ipv6_mode=block` 和引擎路由/浏览器策略阻止；启动探针验证生效值。清单声明 `doh_mode=unsupported` 或 `ipv6_mode=unsupported` 时，代理档案拒绝启动并返回 `KERNEL_UNSUPPORTED`；`unsupported` 清单是合法 fixture，不得静默降级。
- WebRTC 非代理 UDP 由浏览器 `--disable-non-proxied-udp` 阻止；关闭该保护必须经过高级确认并写入 `proxy.non_proxied_udp.disable` 审计事件。
- Hysteria2 的 QUIC/UDP 仅用于引擎到节点，不放宽浏览器非代理 UDP 策略。

## 7. 代理检查

代理检查不得启动第二个 Xray 进程。网关在共享引擎中临时创建：

1. 一个回环 HTTP 检查入站；
2. 一个独立 `chk:<check_id>:out` 临时出站，不写入 `proxy_outbounds`、不增加 `ref_count`；
3. 一条检查路由规则，使用独立 tag `chk:<check_id>:rule`；
4. 通过 HTTP 请求访问配置的回显端点；
5. 无论成功或失败都按 tag 删除临时规则、入站和临时出站。

检查路径不得修改运行档案的 `proxy_outbounds.ref_count`；该计数只表示引用该出站的运行档案数。

检查结果只返回延迟、出口 IP（配置回显时）、地区、错误类和稳定错误码。节点与网络失败以 HTTP 200 `ProxyCheck` 返回（`ok=false`，具体错误在 `error_code`），不得使用未声明的 502；只有认证、授权、校验、存储、引擎未就绪等控制面失败使用 HTTP 错误响应。事件 `proxy.check.completed` 携带同一 `error_code`。

## 8. 故障与重启

- 共享引擎退出：所有运行档案立即失败关闭，发布 `proxy.engine.failed`；网关按有界退避重启。
- 重启后：通过 Xray API 重建所有 `proxy_outbounds`、运行档案入站和路由；入站使用原 `proxy_port`。
- 重建成功：发布 `proxy.engine.rebuilt`；浏览器可恢复网络。
- 重建失败：先把受影响运行时转入 `stopping`，终止内核进程组，删除入站与路由规则，在同一事务中递减出站引用并清空 `proxy_*`，释放配额/锁，teardown 完成后才标记 `failed`；teardown 失败保留 `stopping` 与 `last_error` 并重试；不得存在终态但仍有活内核或残留代理资源，不得直连回退；保留诊断信息。
- 控制面重启：读取 `proxy_engine_state`、`runtimes` 和 `proxy_outbounds`；对非终态 `starting`/`stopping` 行先终止可归属内核、按 tag 删除其代理资源并在同一事务递减引用和清空 `proxy_*`，再转为 `stopped`；teardown 失败保留 `stopping` 与 `last_error`。引用计数只按非终态运行时对账，不一致返回 `PROXY_OUTBOUND_REFCOUNT_MISMATCH`。
- 删除孤儿入站/规则时按 `rt:<id>:*` tag 匹配；不得误删其他运行档案的资源。
- 升级引擎会重置连接池；不得静默替换运行中引擎。

## 9. 安全边界

- Xray API 没有应用层鉴权，只能绑定回环；同用户本机进程的残余风险必须记录。无沙箱服务端模式必须把内核放在独立网络命名空间或等价边界，使管理 API 对内核不可达；无法证明时拒绝启动。桌面端不允许无沙箱启动。
- API 端口与运行档案入站端口分开；API 不映射到容器外。
- 代理凭据不进入日志、事件、错误、argv、浏览器配置或持久化引擎配置。
- 引擎崩溃转储、core dump 和调试日志不得包含凭据；生产环境禁用 core dump。
- 引擎进程以非 root 用户运行；Docker 中最小能力、只读根文件系统。

## 10. 准入确认清单

以下项目必须在 M0 Xray spike 中确认，确认后把结果写入 `docs/DELIVERY-PLAN.md` 或测试报告：

- [ ] `AddInbound` / `RemoveInbound` / `AddOutbound` / `RemoveOutbound` 在 v26.11.9 上可用。
- [ ] `AddRule` / `RemoveRule` 可用；`AddRule` 以 `shouldAppend=true` 增量追加且不清空已有规则，规则带稳定 `ruleTag`，删除只按 tag 且幂等。
- [ ] 引导配置的 `block` 是首出站，启动断言失败时拒绝服务；配置中不存在 `routing.final`、`routing.script` 或 `routing.balancers`。
- [ ] 安装记录保存 `asset_sha256`/`binary_sha256`；启动只校验 `binary_sha256`，缺失与摘要不符分别返回 `PROXY_ENGINE_NOT_FOUND`/`PROXY_ENGINE_CHECKSUM_MISMATCH`。
- [ ] 每运行档案 HTTP 入站只绑定回环；不同档案端口/标签隔离。
- [ ] 沙箱校验失败时网关不创建任何入站、出站或规则。
- [ ] DoH 关闭和 IPv6 阻止的实际开关、能力清单字段与启动探针一致。
- [ ] WireGuard 目标域名在隧道内解析；缺少隧道 DNS 的配置被拒绝，且没有本地 DNS/freedom 回退。
- [ ] 相同 `outbound_key` 的档案共享出站，连接数明显低于每档案独立出站。
- [ ] mux/QUIC 复用能力与协议矩阵一致。
- [ ] Hysteria2 通过 Xray `hysteria` v2 配置可连接；TUIC 被拒绝。
- [ ] 强杀共享引擎后，所有运行档案失败关闭；原端口重建成功。
- [ ] `ref_count` 只统计非终态运行时；teardown 在同一事务中递减并清空四个代理绑定字段，重复 teardown 不重复释放；归零出站删除失败时可作为孤儿重试。
- [ ] 代理检查的节点/网络失败以 HTTP 200 `ProxyCheck.error_code` 返回，不使用 502；临时资源按 tag 删除。
- [ ] 直连模式不创建任何 Xray 入站、出站或规则；服务端开关/审计与降低保证标记生效。
- [ ] 共享引擎 API 不可从非回环地址访问。
- [ ] 代理凭据不出现在 API 响应、日志、事件和持久化配置。

## 11. 参考

- [`specs/proxy-gateway/SPEC.md`](specs/proxy-gateway/SPEC.md)
- [`ARCHITECTURE.md`](ARCHITECTURE.md)
- [`adr/0009-shared-xray-engine.md`](adr/0009-shared-xray-engine.md)
- [`contracts/proxy-engine-manifest.schema.json`](contracts/proxy-engine-manifest.schema.json)
- <https://github.com/XTLS/Xray-core>
