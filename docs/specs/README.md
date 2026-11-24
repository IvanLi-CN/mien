# 规格（Spec）总览

本目录是 topic-level specification catalog：每个 spec 对应一个稳定主题、能力、接口或约束的长期契约，不是单次任务的进度卡片。

每个新 spec 目录包含：

- `SPEC.md`：当前有效规范 / 主题契约；`## Related ADRs` 列出当前关联 ADR，或写 `None`。
- `IMPLEMENTATION.md`：实现覆盖、当前状态、剩余缺口与 rollout 事实。
- `HISTORY.md`：主题局部生命周期、替换、兼容性与必要背景；不复制 ADR 的完整取舍理由。

## 新建 Spec

1. 选择唯一、稳定的 kebab-case topic slug。
2. 新建 `docs/specs/<topic>/`。
3. 创建 `SPEC.md`、`IMPLEMENTATION.md` 与 `HISTORY.md`。
   - `SPEC.md` 的 `## Related ADRs` 按路径升序列出当前关联 ADR 的 Markdown 链接；无关联时写 `None`。
4. 在下方 Index 表尾追加一行，默认 `Lifecycle=active`、`Successor=-`。
5. `SPEC.md` 的 `## Verification` 是契约：每个 `VER-*` 必须通过 `covers` 指向至少一个 `REQ-*`。同目录 `IMPLEMENTATION.md` 必须为每个 `VER-*` 记录验证命令或证据；实现尚未开始时统一标为 `pending` 并写明计划的命令与 fixture，`pending` 是唯一允许的未实现状态，`spec_contract_check` 必须接受它且只对它放行。

新 spec 不使用 ID。历史 `docs/specs/<legacy-id>-<topic>/` 与 `docs/plan/**/PLAN.md` 仅保留读取兼容，规范化由 `$docs-migrate MODE=specs-hygiene` 在确认后完成。

## Lifecycle

- `active`：当前主题 spec 是有效真相源。
- `superseded`：已被 `Successor` 指向的 topic spec 取代。
- `archived`：仅保留历史参考，不再作为当前规范输入。

`Implementation` 只放轻量摘要；详细状态、rollout 与剩余缺口必须写入同目录 `IMPLEMENTATION.md`。

## Index

| Topic | Lifecycle | Implementation | Spec | Successor | Notes |
| - | - | - | - | - | - |
| 环境档案管理 | active | 未开始 | `profile-management/SPEC.md` | - | 档案 CRUD、隔离与下载目录、回收站生命周期、导入导出校验与路径安全 |
| 指纹规格与启动参数 | active | 未开始 | `fingerprint-engine/SPEC.md` | - | 规格、种子、一致性、能力清单与 argv |
| 浏览器运行时与显示环境 | active | 未开始 | `browser-runtime/SPEC.md` | - | 状态机、隔离结论、监督、有头/无头、Xvfb、沙箱与日志尾部 |
| 正向代理协议与网络泄漏防护 | active | 未开始 | `proxy-gateway/SPEC.md` | - | VLESS/VMess/SS/Trojan/Hysteria2/WireGuard、basic HTTP(S)/SOCKS5、共享引擎、分享链接、远程 DNS 与失败关闭 |
| 控制面 API 与鉴权 | active | 未开始 | `control-api/SPEC.md` | - | REST/WS、会话、Token、CSRF 与审计 |
