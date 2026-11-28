# Xray-core v26.11.9 共享引擎 Spike

本 spike 在 macOS arm64 上使用真实 Xray-core v26.11.9 验证 `docs/XRAY-MAPPING.md` 与 `docs/DELIVERY-PLAN.md` §2.1 的共享引擎假设。运行入口是 `./docs/spikes/xray/run.sh`；脚本默认只读 `fixtures/.cache/xray/`，不会下载或提交二进制。

## 复跑

```sh
# 准备缓存后校验资产并解包：
mkdir -p fixtures/.cache/xray/unpack
curl -fL -o fixtures/.cache/xray/Xray-macos-arm64-v26.11.9.zip \
  https://github.com/XTLS/Xray-core/releases/download/v26.11.9/Xray-macos-arm64-v8a.zip
shasum -a 256 fixtures/.cache/xray/Xray-macos-arm64-v26.11.9.zip
unzip -qo fixtures/.cache/xray/Xray-macos-arm64-v26.11.9.zip -d fixtures/.cache/xray/unpack
shasum -a 256 fixtures/.cache/xray/unpack/xray

./docs/spikes/xray/run.sh
```

缓存资产摘要必须为 `233f0c66681cd0726f81a6b2b6ed6a5710869e312047b522e46ed53035bebd26`。本机实测解包可执行文件摘要为 `1b2d5ebe3a4c332203db24b81a052c6d6688a7f80705090f031b7416013ee1b2`；脚本每次运行都重新打印并校验两个摘要，证据日志保存于 `docs/spikes/xray/evidence/run-<UTC>.log`。

## 结论

| 用例 | 结论 | 实测摘要 |
| --- | --- | --- |
| ENGINE-001 版本与校验 | pass | Xray `26.11.9`, Darwin arm64；asset SHA-256 与清单一致；binary SHA-256 `1b2d5ebe…3ee1b2`。最近一次启动耗时实测 `86 ms`。 |
| ENGINE-002 协议矩阵 | pass（TUIC/Hysteria2 范围见下） | VLESS TCP 受控服务端可连接并转发；TUIC 配置被明确拒绝：`unknown config id: tuic`；Hysteria2 v2 配置 schema 被接受。 |
| ENGINE-003 动态配置与就绪 | pass（API 原语）；wrapper 幂等为后续实现责任 | 真实 `HandlerService`/`RoutingService` 完成 Add/Remove Inbound、Outbound、Rule；动态时 outbounds=2（含 block）、inbounds=3、rules=3（含 api）；同端口入站就绪。原始 Xray 重复 Add 会报 `existing tag found`/`duplicate ruleTag`，Remove 未知 inbound 报 `not enough information`，因此网关必须先按 tag 对账并把这些结果映射为幂等。 |
| ENGINE-005 失败关闭 | pass（强杀与成功重建） | `kill -9` 后代理端口不可用，`curl` 无直连回退；按原档案端口重建后恢复，最近一次重建耗时 `110 ms`。本 spike 未覆盖“重建失败后 teardown/failed”控制面状态机。 |
| ENGINE-009 动态 API 与连接复用 | pass（共享出站）；wrapper 幂等为后续实现责任 | 两个档案规则分别命中同一 `out:shared-vless`，API 出站数量保持 1 个共享 VLESS（加 block）；不同凭据产生 `out:distinct-credential`。mux 配置回读为 enabled/concurrency=128（Xray v26.11.9 归一化了请求值 8）；并发请求快照记录 `established_client_tcp_connections=3`。 |

## 必须项实测

- **引导配置**：只有 `api` 回环 dokodemo-door 入站、`block` blackhole 出站和 API 路由；脚本断言 `block` 为首出站、没有 `routing.final`、`routing.script`、`routing.balancers`。
- **流量经过代理**：fixture 仅监听 `127.0.0.1`；VLESS 受控服务端仅监听 `127.0.0.1`，并开启显式入站到 freedom 的路由。两个 profile 的 HTTP 入站请求都返回 fixture 的 `20322` 字节，并在 Xray 日志中显示 `rt:<id>:in -> out:shared-vless`，不是直连成功判定。
- **共享与凭据隔离**：相同 key 的两个 profile 只列出一个共享出站；不同 UUID 列出两个 VLESS 出站。证据日志在退出清理阶段脱敏 UUID。
- **强杀/重建**：强杀共享客户端后 profile 请求失败；无 direct fallback。重建使用原 `PROFILE_A_PORT`，重新加入 outbound/inbound/rule 后 fixture 再次返回 `20322` 字节。
- **TUIC**：v26.11.9 不识别 `tuic` outbound，错误为 `unknown config id: tuic`，应在配置校验阶段拒绝并保留明确原因。
- **Hysteria2**：Xray v26.11.9 使用 `protocol: hysteria`、`version: 2`、`streamSettings.network: hysteria` 的配置 schema 可加载；本机 spike 没有受控 UDP/TLS Hysteria 服务端，因此只证明配置可用，不宣称端到端连通。准入仍需受控 UDP 服务端测试。
- **回环边界**：API 与档案入站均绑定 `127.0.0.1`；从本机 Wi-Fi 地址 `192.168.31.6` 访问 API/profile 端口均连接失败。
- **资源**：最终快照中客户端 Xray RSS `31712 KiB`，受控服务端 RSS `30304 KiB`；API TCP LISTEN 与 profile 原端口 LISTEN 均可见。连接数以 `lsof` 的 established 快照记录，不能等同于长期连接池容量。

## 与映射的差异和后续修复

1. `XRAY-MAPPING.md` 已正确描述 Xray 原生 CRUD 不是幂等：重复 Add 会返回错误、未知 Remove 也会返回错误；网关必须按 tag 读取/列举后复用，并把已存在/未知删除映射为成功。`run.sh` 记录了原始错误，未把 CLI 错误冒充成 Xray 原生幂等。
2. 映射将 Hysteria2 简写为 `hysteria` outbound，但端到端配置还需要 Hysteria transport（`network: hysteria`、`hysteriaSettings`）及 TLS 字段；实现层不能只填 outbound settings。
3. 映射中的 mux 示例 `concurrency: 8` 在 v26.11.9 API 回读为 `concurrency: 128`，说明 Xray 会对该字段归一化；实现和 key 规范化需要采用实际生效值或明确版本策略。
4. 本 spike 未覆盖 Hysteria2 受控 UDP 服务端连通、WireGuard、重建失败 teardown 状态机、API 页面探测和长期 RSS/FD 浸泡；这些保持 blocked，不得以本报告替代。

## 证据

每次运行都会生成一份 UTC 命名日志；日志包含命令输出、版本、两个 SHA-256、API 列表、路由日志、TUIC/Hysteria2 配置结果、强杀/重建、RSS 与 `lsof` 快照。日志不应包含真实凭据；当前 fixture UUID 在退出阶段统一替换为 `<redacted-uuid>`。
