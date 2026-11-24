# 正向代理实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 未开始
- Lifecycle: active
- Catalog note: 协议引擎、类型化代理配置、分享链接与失败关闭契约已建立。

## Implementation Coverage

- Requirement coverage: 计划由 proxy gateway 与固定 Xray shared engine 覆盖 `REQ-PROXY-001` 至 `REQ-PROXY-025`；写锁覆盖引用获取/释放及零引用出站删除，检查临时 `chk:` 资源重启后清理；故障恢复保留活且可归属内核，直连不依赖引擎。
- Verification commands: 尚未建立；实现后使用受控 VLESS/VMess/Shadowsocks/Trojan/Hysteria2/WireGuard 与 basic HTTP/HTTPS/SOCKS5 节点、`asset_sha256`/`binary_sha256` 篡改 fixture、路由 `ruleTag`/`shouldAppend` 断言、blackhole 首出站断言、非终态引用计数与 teardown 序列、`ProxyCheck` HTTP 200 `error_code`、直连服务端开关与审计、DNS canary、引擎故障注入、分享链接 fixture 和凭据扫描。
- Rollout facts: Tier A 为 VLESS、VMess、Shadowsocks、Trojan；Tier B 为 Hysteria2、WireGuard；HTTP/HTTPS/SOCKS5 为 basic tier，不参与 A/B 门禁；TUIC 不进入 v1；订阅属于后续范围。

## Coverage / rollout summary

- 当前尚未记录额外 rollout 事实。

## Remaining Gaps

- 类型化代理配置、共享引擎动态入站/出站/路由（含稳定 `ruleTag` 与 `shouldAppend=true`）、非终态引用计数与 teardown、分享链接解析、远程 DNS、失败关闭、直连模式边界与代理检查均未实现。
- Xray 映射、候选清单与 MPL-2.0 记录已建立；仍需 M0 runtime spike 验证管理 API、`AddRule` 增量语义、blackhole 首出站断言、`binary_sha256` 运行时校验、连接复用、Hysteria2/WireGuard、目标容器 UDP 和故障重建后冻结协议矩阵。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`；当前未实现项为 pending，每行列出计划命令与 fixture。结构检查允许有证据的已实施状态；发布门禁只接受 pass 或有效 waiver。

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-PROXY-001` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-002` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-003` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-004` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-005` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-006` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-007` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-008` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-009` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-010` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-011` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-012` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-013` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-014` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-015` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-016` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-017` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-018` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-019` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-020` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-021` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-022` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-023` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-024` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |
| `VER-PROXY-025` | `pending` | 受控节点集成测试、DNS canary、引擎故障注入、分享链接 fixture；fixture 根 `fixtures/proxy/` |

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../XRAY-MAPPING.md`](../../XRAY-MAPPING.md)
- [`../../contracts/manifests/`](../../contracts/manifests/)
- [`../../ERROR-CODES.md`](../../ERROR-CODES.md)
