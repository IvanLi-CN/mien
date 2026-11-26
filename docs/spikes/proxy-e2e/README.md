# T20 代理链路端到端实测

本 spike 记录 `crates/mien-core/tests/proxy_e2e.rs` 的真实测量：HTTP 请求经过 Mien 共享引擎、
受控 Xray 服务端入站和 `freedom` 出站，到达本地 ECHO 源站；并证明流量确实经过了代理，而不是直连。

结论先行：VLESS、VMess、Shadowsocks、Trojan 四种 TCP 协议在受控节点上完成端到端转发；
错误凭据与节点被杀都不会产生源站流量。同时实测到一个共享引擎生产缺陷（见 §5），
它不阻止上述证据成立，但影响长跑稳定性。

## 1. 复跑

```sh
export XRAY=/Users/ivan/orca/workspaces/mien/t3-xray-spike/fixtures/.cache/xray/unpack/xray

# 收尾基线：必须与结束时一致
ps -axo command | grep -E "mien-runtime-id|xray" | grep -v grep | wc -l

MIEN_XRAY_BIN="$XRAY" cargo test -p mien-core --test proxy_e2e -- --nocapture
cargo run -q -p xtask -- check --skip-host
cargo run -q -p xtask -- fixtures verify

ps -axo command | grep -E "mien-runtime-id|xray" | grep -v grep | wc -l
```

`MIEN_XRAY_BIN` 未设置时每个用例打印 `skipped: MIEN_XRAY_BIN not set` 并返回，
不会静默通过。场景表和期望值在 `fixtures/proxy/e2e-scenarios.json`。

## 2. 链路

```text
测试进程 HTTP 客户端
  -> 127.0.0.1:<proxy_port>          引擎为运行档案分配的 HTTP 入站（Mien）
  -> out:<outbound_key>              共享出站（VLESS/VMess/SS/Trojan，按 key 复用）
  -> 127.0.0.1:<server_port>         受控 Xray 服务端入站（真实鉴权）
  -> direct (freedom)                服务端出站
  -> 127.0.0.1:<echo_port>           ECHO 源站，回显它看到的对端地址
```

三类进程全部是本机真实进程：测试进程内的 tokio ECHO、受控 Xray 服务端、以及由
`EngineProcess::start` 启动的共享引擎。没有 mock、没有桩。

## 3. 区分「走了代理」与「直连」的判据

单一判据都可能有别的解释，因此四条同时成立才算证据；每条都在测试里断言，失败即失败。

| # | 判据 | 直连为何不成立 | 对应断言 |
| --- | --- | --- | --- |
| 1 | 源站主机名 `proxy-e2e-<pid>-<rand>.test` 只存在于**受控服务端自己的** `dns.hosts` 表 | 测试进程 `getaddrinfo` 解析该名失败（每次运行重新生成，不落磁盘，不写 `/etc/hosts`），所以本进程无法直连；引擎引导配置只含 `block` blackhole，没有 direct 出站 | `Target::from_fixture` + `assert_origin_name_unresolvable` |
| 2 | ECHO 回显的**对端端口**既不等于本次直连标定的本进程端口，也不等于客户端到引擎那一跳的端口 | 直连时源站看到的就是发起进程自己的端口（先用同一 ECHO 做直连标定，确认会看到本进程端口） | `assert_ne!(peer_port, direct.local_port)` 与 `assert_ne!(peer_port, proxied.local_port)` |
| 3 | 受控服务端自己的统计 `inbound>>>e2e-srv-in>>>traffic>>>uplink` 增长 | 直连不经过服务端进程，其计数器不动 | `> serverInboundUplinkBytes` |
| 4 | 受控服务端日志出现 `accepted tcp:<源站名>:<端口> [e2e-srv-in -> direct]` | 该行由服务端进程在入站完成鉴权并成功拨号后写出，带的是它自己解析出的源站名；直连不经过该进程，不产生此行 | `wait_for_log` 匹配该行 |

判据 2 是三段式：先用同一 ECHO 做一次**本进程直连标定**，确认源站会如实回显「发起者自己的端口」，
再用同一测量手段看代理请求。标定使「源站看不到真实对端」这类假阳性暴露出来——若回显不可信，
标定就不会等于本进程端口，测试直接失败。

## 4. 每种协议的证据形式

四个协议走同一条断言路径，实测输出（`--nocapture`）形如：

```text
[t20 proxy-e2e] req=REQ-PROXY-001,REQ-PROXY-003,REQ-PROXY-005,REQ-PROXY-021
  scenario=vless-tcp protocol=vless origin=proxy-e2e-60022-60446.test:60462
  client_hop_port=60570 direct_peer_port=60480 proxied_peer_port=60572
  server_uplink_bytes=0->173 server_route=[e2e-srv-in -> direct]
```

| 场景 | 协议 | 判据 2 实测（本进程端口 → 源站看到的端口） | 服务端 uplink 增长 | 服务端拨号行 |
| --- | --- | --- | --- | --- |
| `vless-tcp` | VLESS + mux 关 | `61308` ≠ `61389` | `0 -> 173` 字节 | `accepted tcp:<源站名>:<端口> [e2e-srv-in -> direct]` |
| `vmess-tcp` | VMess（alterId 0） | `61310` ≠ `61349` | `0 -> 351` 字节 | 同上 |
| `shadowsocks-tcp` | Shadowsocks `aes-256-gcm` | `61306` ≠ `61374` | `0 -> 260` 字节 | 同上 |
| `trojan-tcp` | Trojan | `61307` ≠ `61375` | `0 -> 216` 字节 | 同上 |

字节数每次运行不同（端口数字宽度、请求行长度随之变化），断言只要求严格大于 0 且单调增长，
不锁定具体数值。四种协议的出站都由生产代码 `build_outbound` 生成，经真实 gRPC `AddOutbound`
下发；入站和规则同样由 `GatewayAttach`/`EngineController::acquire` 创建。

### 4.1 负向控制

| 用例 | 做法 | 实测 |
| --- | --- | --- |
| 错误凭据（四协议） | 同一引擎、同一服务端，仅换 `rejectedCredentials` | 请求不成功；ECHO 命中数增量为 `0`；服务端日志无 `[e2e-srv-in -> direct]` |
| 节点被杀 | 先确认隧道可用，再 `kill` 受控服务端 | 同一请求失败；ECHO 命中数不变 |

负向控制各自使用独立的引擎生命周期，原因见 §5：两类失败都会让引擎写一条 warning，
而当前 `EngineProcess` 在收到第一条 warning 后会退出，无法在同一引擎里继续断言。

## 5. 已确认的生产缺陷（`EngineProcess` 日志管道）

`crates/mien-core/src/proxy/engine.rs::EngineProcess::start_with_api_port` 在返回前丢弃了
日志通道接收端 `rx`（只有 `logs` 任务持有它，而该任务只 `join!` 两个 pump）。`rx` 一旦被丢弃：

1. pump 里 `t.send(...).await` 返回错误，pump 退出；
2. pump 持有子进程的 `stdout`/`stderr`，pump 结束后这两个管道被关闭；
3. Xray 下一次写日志（任何 warning/error，例如出站连接失败、鉴权失败）命中已关闭管道，
   收到 `SIGPIPE` 退出。

实测三条独立证据：

| 证据 | 方法 | 结果 |
| --- | --- | --- |
| 原始进程复现 | 直接 `Popen(xray)`，读一小段后关闭两条管道，再触发一条 warning | 进程 `poll()` 返回 `-13`（`SIGPIPE`） |
| Mien 路径复现 | `EngineProcess::start` 后经运行档案入站发一次必然失败的请求 | `api().probe()` 从 `Ok` 变成 `Err(Unavailable "tcp connect error")`，`ps` 显示 `Z <defunct>` |
| 只改变量对照 | 同样的请求，但持续 drain 两条管道 | 引擎存活，`probe()` 为 `Ok` |

影响：共享引擎在第一次写 warning/error 日志后即死。生产中这会表现为
「第一个出站失败的运行档案之后，整个代理网关失效」——恰恰是 REQ-PROXY-009/022
要求失败关闭并能重建的场景。该文件属于 `crates/mien-core/src/**`，超出本任务授权，
**未修改**；需要另行修复（建议把 pump 的读端所有权移出 startup 作用域，或在 pump 结束后
把子进程 stdout/stderr 重定向到 `/dev/null`，并让 `logs` 任务常驻直到 `stop()`）。

因此本测试的断言被刻意组织成「每个引擎生命周期内只发生一次终止性事件」：
正向用例在引擎健康时完成全部证据和 teardown 断言，两种负向控制各起一个新引擎。

## 6. 未能证明的项

| 项 | 状态 | 原因 |
| --- | --- | --- |
| Hysteria2 端到端 | 未证明 | 需要受控 UDP/TLS 服务端；本轮只覆盖 TCP |
| WireGuard 端到端 | 未证明 | 需要受控 WireGuard peer 与宿主机路由，属容器环境 |
| WS / gRPC / TLS / REALITY 传输变体 | 未证明 | 本轮场景固定 `tcp` + 无 TLS |
| SOCKS5 / HTTP(S) 基础协议 | 未证明 | 只覆盖 Tier A 四协议 |
| 浏览器内核（Chromium）发起流量 | 未证明 | 本测试用进程内 HTTP 客户端等价替代；浏览器侧另有 `network_canary` 覆盖 |
| 连接复用比例（mux 效果） | 未证明 | 本轮四协议均未开 mux；复用计数需要并发慢请求快照，未纳入 |
| DNS 泄漏边界 | 未证明 | 源站名解析只发生在受控服务端；本机黑洞化 DNS 观测未在本测试内做 |

上述项继续按 `docs/XRAY-MAPPING.md` 的冻结边界处理：未实测不得推导为可用能力。

## 7. 证据

- 测试源码：`crates/mien-core/tests/proxy_e2e.rs`
- 场景与期望：`fixtures/proxy/e2e-scenarios.json`
- 本文件 `--nocapture` 输出片段见 `evidence/proxy-e2e.log`（脱敏：UUID 与口令替换为占位符）
