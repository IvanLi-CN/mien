# Xray 共享协议引擎映射

本文把 `docs/specs/proxy-gateway/SPEC.md` 的代理契约落到 Xray-core v26.11.9 的进程、API、入站、出站、路由、连接复用和故障恢复。它是实现前的确定性映射；标为“准入确认”的项目必须在 M0 Xray spike 中验证后才能冻结。

## 1. 进程与版本

- 每个宿主一个 Xray-core 进程，版本固定为 `v26.11.9` 或经准入的后继补丁版本。
- 二进制校验 SHA-256；不修改二进制。
- 管理 API 只监听 `127.0.0.1` 的随机端口；不映射到容器外或局域网。
- 代理凭据只通过回环管理 API 或受保护引导通道传递，不进入 argv、日志、事件、持久化配置或浏览器。
- 浏览器只连接本运行档案的回环 HTTP 入站。

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
    ],
    "final": "block"
  }
}
```

实现时由代理网关分配实际 API 端口并写入引导配置；`port: 0` 只是合同示意，不能让 Xray 自行分配后无法被网关发现。

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
- 运行档案停止：删除其路由规则和入站，`ref_count--`。
- 引用归零：`RemoveOutbound`，删除 `proxy_outbounds` 行。
- API 调用必须串行化；启动、失败回滚和引擎重建都要恢复引用计数。

## 5. 协议映射

下表是首版必须实现的映射。字段名以 Xray v26.11.9 的实际 JSON schema 为准；标为“准入确认”的组合必须在 spike 中用 `sing-box` 不参与，直接用 Xray 配置验证。

| Mien 协议 | Xray protocol | 关键 settings / stream | 备注 |
| --- | --- | --- | --- |
| HTTP / HTTPS | `http` | server、port、用户名密码；TLS 按配置 | 认证只在出站 |
| SOCKS5 | `socks` | server、port、用户名密码 | 域名地址优先 |
| VLESS | `vless` | `vnext` 用户 ID、flow；network、TLS/REALITY、WS/gRPC | 不把 UUID 写入浏览器 |
| VMess | `vmess` | `vnext` 用户 ID、security；network、TLS、WS/gRPC | AEAD |
| Shadowsocks | `shadowsocks` | server、port、method、password | 2022 方法按清单 |
| Trojan | `trojan` | server、port、password；TLS、WS | TLS 必须 |
| Hysteria2 | `hysteria` | `version: 2`、address、port；hysteria transport、auth、TLS | 需要 UDP 出站 |
| WireGuard | `wireguard` | peer 公钥、私钥、endpoint、allowed IPs | Tier B，按清单 |

共享 constraints：

- 所有出站默认启用 TCP 连接复用/多路复用（协议支持时）；Hysteria2 使用其原生 QUIC 连接复用，不假装可被 Xray mux 覆盖。
- 出站只允许一个目标节点；不支持多节点 selector 或自动故障切换。
- 不生成 `freedom`/direct 出站给浏览器流量；默认 `block`。
- 不启用 Clash API、远程控制或订阅更新。

## 6. 路由与 DNS

- 每个运行档案一条路由规则：`inboundTag: ["rt:<id>:in"]` → `outboundTag: "out:<key>"`。
- 路由规则使用 `shouldAppend=false`，确保运行档案规则在默认 block 之前；准入确认 Xray v26.11.9 的 `AddRule` 顺序。
- 默认路由为 `block`；未匹配、引擎故障或规则缺失时必须断网，不回退直连。
- 目标域名由 Xray 出站或代理内 DNS 解析；只有代理节点主机名允许本地 bootstrap 解析。
- DNS-over-HTTPS 绕过被禁用；IPv6 无法代理时阻止；WebRTC 非代理 UDP 由浏览器 `--disable-non-proxied-udp` 阻止。
- Hysteria2 的 QUIC/UDP 仅用于引擎到节点，不放宽浏览器非代理 UDP 策略。

## 7. 代理检查

代理检查不得启动第二个 Xray 进程。网关在共享引擎中临时创建：

1. 一个回环 HTTP 检查入站；
2. 一个临时出站（或复用相同 key 的出站，并临时增加引用）；
3. 一条检查路由规则；
4. 通过 HTTP 请求访问配置的回显端点；
5. 无论成功或失败都删除临时规则和入站，并释放出站引用。

检查结果只返回延迟、出口 IP（配置回显时）、地区、错误类和稳定错误码。

## 8. 故障与重启

- 共享引擎退出：所有运行档案立即失败关闭，发布 `proxy.engine.failed`；网关按有界退避重启。
- 重启后：通过 Xray API 重建所有 `proxy_outbounds`、运行档案入站和路由；入站使用原 `proxy_port`。
- 重建成功：发布 `proxy.engine.rebuilt`；浏览器可恢复网络。
- 重建失败：受影响运行时标记 `failed`，不得直连回退；保留诊断信息。
- 控制面重启：读取 `proxy_engine_state`、`runtimes` 和 `proxy_outbounds`；确认引擎归属后接管或重建。
- 删除孤儿入站/规则；不得误删其他运行档案的资源。
- 升级引擎会重置连接池；不得静默替换运行中引擎。

## 9. 安全边界

- Xray API 没有应用层鉴权，只能绑定回环；同用户本机进程的残余风险必须记录。
- API 端口与运行档案入站端口分开；API 不映射到容器外。
- 代理凭据不进入日志、事件、错误、argv、浏览器配置或持久化引擎配置。
- 引擎崩溃转储、core dump 和调试日志不得包含凭据；生产环境禁用 core dump。
- 引擎进程以非 root 用户运行；Docker 中最小能力、只读根文件系统。

## 10. 准入确认清单

以下项目必须在 M0 Xray spike 中确认，确认后把结果写入 `docs/DELIVERY-PLAN.md` 或测试报告：

- [ ] `AddInbound` / `RemoveInbound` / `AddOutbound` / `RemoveOutbound` 在 v26.11.9 上可用。
- [ ] `AddRule` / `RemoveRule` 可用，且 `shouldAppend=false` 的规则顺序符合默认 block 预期。
- [ ] 每运行档案 HTTP 入站只绑定回环；不同档案端口/标签隔离。
- [ ] 相同 `outbound_key` 的档案共享出站，连接数明显低于每档案独立出站。
- [ ] mux/QUIC 复用能力与协议矩阵一致。
- [ ] Hysteria2 通过 Xray `hysteria` v2 配置可连接；TUIC 被拒绝。
- [ ] 强杀共享引擎后，所有运行档案失败关闭；原端口重建成功。
- [ ] 共享引擎 API 不可从非回环地址访问。
- [ ] 代理凭据不出现在 API 响应、日志、事件和持久化配置。

## 11. 参考

- [`specs/proxy-gateway/SPEC.md`](specs/proxy-gateway/SPEC.md)
- [`ARCHITECTURE.md`](ARCHITECTURE.md)
- [`adr/0009-shared-xray-engine.md`](adr/0009-shared-xray-engine.md)
- [`contracts/proxy-engine-manifest.schema.json`](contracts/proxy-engine-manifest.schema.json)
- <https://github.com/XTLS/Xray-core>
