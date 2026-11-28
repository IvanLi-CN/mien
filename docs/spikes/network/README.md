# T16 网络防护证据：DoH / IPv6 / 非代理 UDP / WebRTC

本 spike 在 macOS arm64（Chromium 148.0.7778.215，`fingerprint-chromium`）以 **headful direct**
模式按产品路径启动真实内核，并通过 CDP 在本机探针页上直接测量四件事：目标域名是否在本地解析、
WebRTC 是否产生直连候选、IPv6 是否真的离开主机、以及浏览器自己的解析器是否启用了 DoH。

结论以「能力清单声明 ↔ 实测观察」对账给出，允许结论是 `Unverified`。任何未观测到的东西都不会
被写成 `Declared`。

证据文件：`evidence/network-canary.json`（本次运行的脱敏结构化结论）。原始 `--nocapture`
输出不提交，因为其中含本机全局 IPv6 地址与临时端口。

## 复跑

```sh
MIEN_KERNEL_BIN=/path/to/Chromium \
  cargo test -p mien-core --test network_canary -- --nocapture
```

`MIEN_KERNEL_BIN` 未设置时测试打印 `skipped: MIEN_KERNEL_BIN not set` 并返回。测试自身启动
UDP+TCP DNS 桩、ECHO 端点与探针页，全部绑定回环；只有 IPv6 那一步会主动访问本机全局地址。

内核 argv 来自 `build_launch_argv_for_mode(..., Headful)`，只额外追加测量通道
（`--remote-debugging-port=0`、`--log-net-log`、`--user-data-dir`、`--mien-runtime-id`）以及
主分支修复后的 `--use-mock-keychain`、`--password-store=basic`。网络控制没有被改写。

## 结论

| 控制 | 清单声明 | 实测 | 对账结果 | 证据 |
| --- | --- | --- | --- | --- |
| `doh_mode` | `disabled`（`Declared`） | `secure_dns_mode=0`、无 DoH 服务器、`can_use_secure_dns_transactions=false`、无 DoT；17 次浏览器解析全部 `secure_dns_policy=0` | **`Declared` 保持** | 内核自己的 netlog：`polledData[].hostResolverInfo.dns_config` 与逐条 `HOST_RESOLVER_MANAGER_REQUEST` |
| `ipv6_mode` | `block`（`Declared`） | 直连下访问本机全局 IPv6 `240e:37a:…` 成功，对端看到的就是该全局地址 | **降为 `Unverified`** | 对端 ECHO 记录的 `peer` |
| `disable_non_proxied_udp` | `true`（`Unverified`） | 只产生两条 `tcp` host 候选（`0.0.0.0:9`、`[::]:9`），**UDP 候选为 0**，`iceGatheringState=complete` | **`Unverified` 不变** | `RTCPeerConnection` 候选集合与 `getStats` |
| DNS canary | 代理模式下远程 DNS | `*.canary.test` 访问失败；本地桩 0 查询，netlog 0 条解析器请求 | `Inconclusive`（见下文原因） | DNS 桩计数 + netlog `HOST_RESOLVER_MANAGER_REQUEST` |

逐项说明：

- **DoH 保持 `Declared`，但只是当前构建 + 当前系统状态的结论。** 内核的 resolver 配置里
  `secure_dns_mode=0`、DoH 服务器列表为空、安全事务不可用、DoT 未生效，实测解析全部带
  `secure_dns_policy=0`，所以 DoH 不会绕过代理。系统偏好一旦打开 DoH，这个结论会变（见对照）。
- **IPv6 必须降级。** 清单写 `block`，但没有任何 IPv6 阻断开关；直连下 IPv6 走真实出口。代理模式
  下能否真的阻断取决于之后加入的代理控制，本任务只记录事实。
- **WebRTC 判不了。** 该内核在本机完全不产生 UDP 候选（连 `typ host` 的 UDP 也没有，只有
  `tcp 0.0.0.0:9` 与 `tcp [::]:9`），因此「无非代理 UDP」既不能被证实也不能被推翻。
- **DNS 桩在 macOS 上看不到流量。** 详见「DNS 通道为什么停在 `Inconclusive`」：三种强制手段
  在本机都失效，netlog 只有请求没有结果事件。

## 实测明细

```text
kernel sha256=d918c691ce56648bc555f8f399c29fc83eb02224165a086491688d01707170cb
global_ipv6=Some(240e:37a:abb7:6700:9939:72b4:6586:a148)
dns: attempt=error:TypeError: Failed to fetch stub_queries=0 netlog_queries=0 verdict=Inconclusive
echo: v4=ok:{"peer":"127.0.0.1",...} v6=ok:{"peer":"::1",...}
webrtc: candidates=0 input=[] state=complete stats=2 verdict=Inconclusive
ipv6: observed_route=Some("240e:37a:abb7:6700:9939:72b4:6586:a148") verdict=Reachable
doh: secure_dns_mode=0 doh_servers=0 secure=false dot=false request_policies=[0 x17] -> Disabled
reconciled: doh=Declared ipv6=Unverified webrtc=Unverified dns=Inconclusive
```

## 对照运行（证明探针能看见泄漏方向）

只看「没观测到」不足以区分「没有泄漏」和「观测不了」。两个对照把方向反转：

| 对照 | 做法 | 预期 | 实测 |
| --- | --- | --- | --- |
| DoH 反转 | `Local State` 预置 `dns_over_https.mode=secure` + 合成模板 | 解析器状态变 `Enabled` | `secure_dns_mode=2`、`doh_servers=1`、`can_use_secure_dns_transactions=true` → `Enabled`，断言通过 |
| WebRTC | 同一探针跑系统 Chrome | 必须收集到直连候选 | 4 条候选、`Direct`，断言通过 |

两个对照都由测试自己断言：主测试读到 `Disabled` / `Inconclusive` 因此不是「读不到」。

`Local State` 预置只改解析器配置，不改 argv；把 `doh_mode=disabled` 变成产品保证仍然需要
`argv.rs` 的改动，本任务没有动它。

### DNS 通道为什么停在 `Inconclusive`

本机 macOS 的页面解析交给 mDNSResponder，实测三种强制手段都不把页面解析打到本地桩：

- `--host-resolver-rules=MAP * …`：内核主进程长时间不开放 CDP 端点，页面请求也打不出来；
- `--host-resolver-rules=MAP <name> …`：页面请求带显式端口，绕过映射，桩 0 查询；
- `--dns-server=127.0.0.1:<桩端口>` 单独使用：桩 0 查询。

netlog 里只有 `HOST_RESOLVER_MANAGER_REQUEST`（主机名 + `secure_dns_policy`）而没有结果事件。
因此 DNS 判定的可信部分是「浏览器有没有把目标名交给自己的解析器」，不是「谁答的」。代理模式下
远程 DNS 仍须由节点侧验证，本任务不声称已证明。

## 判定规则

规则在 `crates/mien-core/src/network/mod.rs`，由 `fixtures/dns/canary-scenarios.json` 与
`fixtures/network/canary-scenarios.json` 的合成场景覆盖全部三态：

- **DNS**：观测到目标名才算 `Leaked`；观测到查询但没有目标名才是 `NotLeaked`；一条都没看到是
  `Inconclusive`。
- **WebRTC**：`typ host`/`srflx`/`prflx` 或任何 UDP 候选都是直连；只有 `typ relay` 与「无非代理
  UDP」相容；候选为空是 `Inconclusive`。
- **IPv6**：对端看到的非回环 IPv6 才算出口；`::1` 不算；没尝试是 `Inconclusive`。
- **DoH**：`secure_dns_mode != 0`、配置了 DoH 服务器、安全事务可用、DoT 生效，任一成立即
  `Enabled`；实际解析落在非 0 `secure_dns_policy` 上也算 `Enabled`；两者都读不到才是
  `Inconclusive`。

对账只降不升：`Declared` 只有在观察支持时才保持，否则退为 `Unverified`。

## 平台边界

- 只在 macOS arm64 与上述内核 build 上验证；Linux/Windows 同源清单未复测。
- 本机 IPv6 出口依赖宿主网络（当前有全局 IPv6）；无 IPv6 的主机上该列恒为 `Inconclusive`。
- 回环 ECHO 只是控制项，回环可达不代表公网可达。
- 证据只含合成名字与地址，不含真实解析日志、凭据或 system profile。
