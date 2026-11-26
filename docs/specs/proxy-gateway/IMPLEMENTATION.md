# 正向代理实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 部分实现（M2）：共享 Xray 进程监督、gRPC 动态入站/出站/路由、引用计数、分享链接解析与凭据加密存储已落地；`ProxyCheck`、故障重建的控制面终态、Tier B 端到端与代理 UI 未实现。
- Lifecycle: active
- Catalog note: 协议引擎、类型化代理配置、分享链接与失败关闭契约已建立。

## Implementation Coverage

- Requirement coverage: proxy gateway 与固定 Xray shared engine 覆盖 `REQ-PROXY-001` 至 `REQ-PROXY-025` 的子集：`EngineProcess` 摘要校验与回环 block-first 引导、`EngineController` 的 `acquire/release/crash/rebuild` 与出站引用计数、`outbound_key` 归一化（有效 mux 并发 128）、Hysteria2 传输/TLS 要求、TUIC 拒绝、`GatewayAttach` 按运行时绑定回环入站，以及 `parse_share_link` 的五协议草稿解析。
- Verification commands: 可复跑证据见 `## M2 slice evidence`；除此之外没有命令可覆盖整行 pass condition 的条目一律保持 `pending`。
- Rollout facts: Tier A 为 VLESS、VMess、Shadowsocks、Trojan；Tier B 为 Hysteria2、WireGuard；HTTP/HTTPS/SOCKS5 为 basic tier，不参与 A/B 门禁；TUIC 不进入 v1；订阅属于后续范围。

## Coverage / rollout summary

- 真实 Xray v26.11.9 上已验证：共享出站复用、动态入站与规则的幂等 Add/Remove、强杀后按原 API 端口重建、TUIC 稳定拒绝、VMess/Shadowsocks/Trojan 出站装配。
- 尚无受控代理节点：所有集成测试的 `server` 都是 `127.0.0.1`，因此“经代理访问探针页”“远程 DNS”“出口 IP”都没有实测。

## Remaining Gaps

- `ProxyCheck`（含 HTTP 200 + `error_code` 语义）完全未实现，`VER-PROXY-013` 无对象可测。
- 代理故障的运行时侧终态（保留 `running`、按原端口重建、失败后 `stopping → failed`）只在引擎层有单元证据，运行时层未接线。
- `proxy_outbounds` 表只有迁移，没有写入路径；“同一事务递减引用并清空 `proxy_*`”、`chk:` 残留清理与写锁互斥都未实现。
- 非代理 UDP 的高级关闭不存在对应 argv 开关（仓库中无 `--disable-non-proxied-udp`），`VER-PROXY-010` 无法闭合。
- 代理页面仍是占位 `<Surface task="T9-C">`，清单驱动的协议/传输选择与升级差异比较未实现。
- Tier B（Hysteria2/WireGuard）只有配置校验，没有端到端；HTTP/HTTPS/SOCKS5 basic tier 同样未实测。
- 管理 API 的 LSM/防火墙独占策略、服务端沙箱与直连门禁（`MIEN_ALLOW_DIRECT_EGRESS`）都未实现。
- 注意启动顺序：`StartProfile::start` 先 `proxy.attach(...)` 再做沙箱探测，失败时靠 `detach` 回滚；这与“沙箱校验失败时不创建任何代理资源”不同，`VER-PROXY-003` 因此不能判 pass。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`。本表的 `pass` 只表示“仓库内存在可运行命令且 T19 亲自运行通过”，并写出确切命令；其余保持 `pending` 并写明缺什么。发布门禁只接受 pass 或有效 waiver。

复跑环境（下文命令引用这两个变量）：

```sh
export KERNEL=/Users/ivan/orca/workspaces/mien/t4-kernel-spike/fixtures/.cache/kernel/148/macos-arm64/Chromium.app/Contents/MacOS/Chromium
export XRAY=/opt/homebrew/bin/xray
```

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-PROXY-001` | `pending` | 真实 Xray 上 VLESS/VMess/Shadowsocks/Trojan 出站可装配并释放（`MIEN_XRAY_BIN="$XRAY" cargo test -p mien-core --test xray_integration`），但没有受控节点与浏览器探针页，Tier B 与 basic 未实测。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-002` | `pending` | 未声明协议/传输、缺失凭据、未知加密方法已有稳定码（`cargo test -p mien-core --lib proxy::api`、`--lib proxy::keys`）；缺错误端口（0/越界）与超长字段用例，代理输入也不拒绝未知字段。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-003` | `pending` | 引用计数共享与释放有单元与真实引擎证据，但“全宿主只有一个引擎”无断言，且 `StartProfile` 先 attach 代理再探测沙箱，不满足“沙箱校验失败时不创建任何代理资源”。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-004` | `pending` | `verify_binary_sha256` 会返回 `PROXY_ENGINE_NOT_FOUND`/`PROXY_ENGINE_CHECKSUM_MISMATCH`，但没有对应测试；“安装阶段校验 `sha256` 与 `asset_sha256` 一致”无实现。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-005` | `pending` | 规则 tag 稳定、同 key 单出站、引导无 direct 出站均已断言（`--test xray_integration`、`--lib proxy::gateway`）；缺“入站协议为 HTTP 且仅监听回环”的断言，也没有未匹配流量的失败关闭实测。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-006` | `pending` | 凭据不序列化、不入库明文（`--lib proxy::keys`、`--test proxy_storage`）；缺 argv/日志/事件的扫描断言，管理 API 的 LSM/防火墙独占策略未实现。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-007` | `pending` | `--test slice_e2e` 断言浏览器 argv 只带 `--proxy-server=http://127.0.0.1:<port>`；未断言 argv 中不存在 UUID/密码/私钥/REALITY 秘密，也未做 profile 配置扫描。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-008` | `pending` | 没有受控节点与远程 DNS 断言；`network_canary` 只测直连 headful，DNS 判定停在 `Inconclusive`。fixture 根 `fixtures/proxy/`、`fixtures/dns/` |
| `VER-PROXY-009` | `pending` | 引擎失败关闭在单元层有证据（`crash_fails_closed_and_keeps_bindings_for_replay`）；缺错误凭据/断网/非法配置的浏览器侧失败断言与 UI 错误类。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-010` | `pending` | argv 中不存在 `--disable-non-proxied-udp`；高级关闭确认、审计与风险标记均未实现。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-011` | `pending` | 无启用 IPv6 的容器与 IPv6-only 目标实测；`docs/spikes/network` 只给出直连 IPv6 可达的反例。fixture 根 `fixtures/proxy/`、`fixtures/network/` |
| `VER-PROXY-012` | `pending` | DoH 只在直连 headful 下实测为 `Disabled`（`docs/spikes/network`），代理链路未测；无“无法验证时拒绝启动”的接线。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-013` | `pending` | `ProxyCheck` 未实现（仓库中无该类型与端点），HTTP 200 + `error_code`、临时资源清理都无从验证。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-014` | `pending` | `api_error` 把引擎错误映射为稳定码，但没有“每种失败返回不同码”的用例矩阵。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-015` | `pass` | `cargo test -p mien-core --test share_link`（8 passed）：30+ fixture 用例覆盖 vless/vmess/ss/trojan/hysteria2 的合法草稿、缺失字段与超大/畸形输入，解析为纯函数，不发起网络请求、不写库、不启动。fixture 根 `fixtures/proxy/share-links.json` |
| `VER-PROXY-016` | `pending` | `ProxyView` 只含 `credential_set`/`credential_fields` 且无明文（`--test proxy_storage` 的 `create_get_and_roundtrip_through_storage`）；但核心仍提供 `load_secret` 明文读回，且无 UI/API 响应扫描与分享链接残留断言。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-017` | `pending` | `GrpcXrayApi::connect` 拒绝非回环地址（`--lib proxy::api` 的 `loopback_only`）；缺运行结束后入站端口关闭的断言、服务端网络边界与页面回环探测。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-018` | `pending` | 直连不建入站/出站并有降低保证说明（`--test slice_e2e` 的 `slice_starts_stops_and_restarts`）；`DIRECT_EGRESS_DISABLED` 只有码表，服务端门禁与审计未实现。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-019` | `pending` | 运行时日志有脱敏与有界尾部（`--lib runtime::logs`），代理引擎自身日志只被 drain、无轮转与内容断言。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-020` | `pending` | 引擎清单通过 schema 校验（`xtask fixtures verify`、`xtask contracts`）；代理页面为占位组件，未声明组合的 UI 拒绝与升级差异比较未实现。fixture 根 `fixtures/engine/manifest.json` |
| `VER-PROXY-021` | `pending` | 同 key 共享/异凭据不共享、重复释放不重复递减均有证据（`--test xray_integration`、`--lib proxy::engine`）；“同一事务递减并清空 `proxy_*`”未实现，`proxy_outbounds` 只有迁移。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-022` | `pending` | 引擎强杀后按原 API 端口重建、阶梯耗尽写 `failed`、绑定在崩溃后保留均有证据；缺运行时层“保留 `running` 与原端口/清理失败保留非终态”的端到端断言。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-023` | `pending` | 无控制面崩溃接管、`chk:` 残留清理、PID 复用分类与引用对账的死内核/活引擎矩阵。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-024` | `pending` | `ensure_rule` 以 `shouldAppend=true` 添加、规则 tag 稳定（fake 断言 `a==true`）；缺规则删除幂等的单测与并发添加/删除的顺序断言，`routing.final` 只在引导校验中被拒。fixture 根 `fixtures/proxy/` |
| `VER-PROXY-025` | `pending` | 引导配置 block 首出站、错误引导被 `verify_bootstrap_config` 拒绝有单测（`--lib proxy::gateway`）；`EngineController::assert_bootstrap` 无生产调用者，“断言失败时不服务任何运行档案”无证据。fixture 根 `fixtures/proxy/` |

## M1 slice evidence

- Delivered: Xray process supervision with binary digest verification, gRPC `HandlerService`/`RoutingService` client, block-first loopback bootstrap, gateway-level idempotent reconciliation, shared outbound refcounting, `outbound_key` normalization (effective mux concurrency 128), Hysteria2 transport/TLS requirements, and TUIC rejection. `GatewayAttach` binds a runtime to a loopback inbound and releases by runtime id.
- Commands: `cargo test -p mien-core --test xray_integration` and `tests/slice_e2e.rs` with `MIEN_XRAY_BIN` set to Xray v26.11.9.
- Not delivered yet: Tier A protocol matrix beyond VLESS (VMess/Shadowsocks/Trojan), share-link import, remote DNS canary, Hysteria2/WireGuard end-to-end. Their VER rows stay `pending`.

## M2 slice evidence

- Re-run in T19 on 2026-11-27, Xray 26.11.9 (`/opt/homebrew/bin/xray`), macOS 27.2 / arm64:
  - `MIEN_XRAY_BIN="$XRAY" MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test xray_integration --test slice_e2e` — 6 + 4 passed（共享出站引用、动态规则、按原 API 端口重建、TUIC 拒绝、运行时绑定与释放）。
  - `cargo test -p mien-core --test share_link` — 8 passed（五协议草稿与拒绝矩阵）。
  - `cargo test -p mien-core --lib proxy::api`、`--lib proxy::gateway`、`--test proxy_storage` — 8 / 4 / 11 passed。
- Only `VER-PROXY-015` has evidence covering its whole pass condition; the rest stay `pending` for the reasons above.

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../XRAY-MAPPING.md`](../../XRAY-MAPPING.md)
- [`../../contracts/manifests/`](../../contracts/manifests/)
- [`../../ERROR-CODES.md`](../../ERROR-CODES.md)

最近同步：M2（T19），依据 `MIEN_XRAY_BIN="$XRAY" MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test xray_integration --test slice_e2e`、`cargo test -p mien-core --test share_link`、`cargo run -q -p xtask -- check --skip-host`。
