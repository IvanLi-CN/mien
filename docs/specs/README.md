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
5. SPEC Verification 中每个 VER-* 的 covers 必须指向至少一个有效 REQ-*；IMPLEMENTATION 为每个 VER 记录状态、命令与 fixture/证据。状态闭集为 pending、pass、fail、blocked：pending 只表示未实现且必须有计划命令/fixture；其余必须有实际命令、结果和证据（blocked 另有阻断原因）。结构检查接受上述完整记录，不能把“未实现状态只允许 pending”误解为“只有 pending 可过”；运行/发布门禁仍要求适用 VER pass，fail/blocked 不可冒充通过，文档阶段 pending 不表示已验证实现。

新 spec 不使用 ID。历史 `docs/specs/<legacy-id>-<topic>/` 与 `docs/plan/**/PLAN.md` 仅保留读取兼容，规范化由 `$docs-migrate MODE=specs-hygiene` 在确认后完成。

## Lifecycle

- `active`：当前主题 spec 是有效真相源。
- `superseded`：已被 `Successor` 指向的 topic spec 取代。
- `archived`：仅保留历史参考，不再作为当前规范输入。

`Implementation` 只放轻量摘要；详细状态、rollout 与剩余缺口必须写入同目录 `IMPLEMENTATION.md`。

## Index

| Topic | Lifecycle | Implementation | Spec | Successor | Notes |
| - | - | - | - | - | - |
| 环境档案管理 | active | M0 契约冻结，实现 pending | `profile-management/SPEC.md` | - | 档案 CRUD、隔离与下载目录、回收站生命周期、导入导出校验与路径安全 |
| 指纹规格与启动参数 | active | M0 契约冻结，实现 pending | `fingerprint-engine/SPEC.md` | - | 规格、种子、一致性、能力清单与 argv |
| 浏览器运行时与显示环境 | active | M0 契约冻结，实现 pending | `browser-runtime/SPEC.md` | - | 状态机、隔离、监督、有头/无头、Xvfb、沙箱与日志尾部 |
| 正向代理协议与网络泄漏防护 | active | M0 契约冻结，实现 pending | `proxy-gateway/SPEC.md` | - | VLESS/VMess/SS/Trojan/Hysteria2/WireGuard、basic HTTP(S)/SOCKS5、共享引擎、分享链接、远程 DNS 与失败关闭 |
| 控制面 API 与鉴权 | active | M0 契约冻结，实现 pending | `control-api/SPEC.md` | - | REST/WS、会话、Token、CSRF 与审计 |
