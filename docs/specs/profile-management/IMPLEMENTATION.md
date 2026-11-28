# 环境档案管理实现状态

> 当前有效规范以 `./SPEC.md` 为准；本文件记录实现覆盖、交付状态与 rollout 事实。

## Current Status

- Implementation: 部分实现（M2）：SQLite 档案仓储（创建/读取/列表/更新/归档/恢复/软删除/清理/复制）、`profiles/<id>` 与 `trash/<id>` 目录、路径越界拒绝与 `proxy_id` 绑定已落地；导入导出、分组标签 API、下载目录 UI 绑定与内核迁移未实现。
- Lifecycle: active
- Catalog note: 开发前契约已建立，核心存储已落地，等待 API 层实现。

## Implementation Coverage

- Requirement coverage: `mien-core` 的 profile 领域模块、SQLite 迁移 0002/0003 与 `SqliteProfileRepository` 覆盖 `REQ-PROFILE-001` 至 `REQ-PROFILE-015` 的存储子集；控制面端点尚未实现，桌面侧通过 Tauri IPC 暴露等价的 create/list/get/update/runtime 命令。
- Verification commands: 可复跑证据见 `## M2 slice evidence`；导入导出、分组标签与内核迁移没有对应命令，相关 VER 保持 `pending`。
- Rollout facts: 无运行时数据；SQLite schema、迁移规则与错误码已建立，首个实现切片负责落迁移和 fixture；`profiles.deletion_error`、下载目录与 `trash/<id>` 目录布局随首个迁移落地。

## Coverage / rollout summary

- 四个迁移幂等可重跑，`PRAGMA foreign_key_check` 为空、`integrity_check=ok`，`schema_migrations` 版本为 4。
- `downloads` 目录随档案创建，但权限是进程 umask 默认值（非 0700），且 Chromium 侧没有 `--download-default-directory` 之类的绑定。

## Remaining Gaps

- 导入（含 `import/validate` 零写入 dry-run、冲突策略、加密信封、Argon2id）与导出（plain/encrypted 信封）完全未实现；`profile-export.schema.json` 只有契约。
- 分组与标签只有表，无仓储方法与 API；`groups.name`/`tags.name` 的唯一约束存在，但重复关联的幂等行为没有实现路径。
- 生命周期缺 `deletion_error` 的重试入口，也没有“移动后事务提交失败则目录移回”的测试；目录删除失败只写一次 `deletion_error`。
- 缺少 0700 权限实现；`validate_relative_path` 只守护目录解析，不设置权限位。
- 每档案内核迁移（预检/审计/回滚）与 `copy_fingerprint` 之外的导入种子保留均未实现。
- 运行时状态不落库，因此“运行中档案删除返回 `RUNTIME_CONFLICT`”无法实现。

## Verification Status

Verification Status 使用 `pending|pass|fail|blocked`。本表的 `pass` 只表示“仓库内存在可运行命令且 T19 亲自运行通过”，并写出确切命令；其余保持 `pending` 并写明缺什么。发布门禁只接受 pass 或有效 waiver。

`slice_e2e` 一行需 `MIEN_KERNEL_BIN`，其余命令无需额外变量；`KERNEL` 指向 `t4-kernel-spike` 缓存的 pinned Chromium 148（见 `docs/spikes/runtime/README.md`）。

| VER | 状态 | 计划命令与 fixture |
| --- | --- | --- |
| `VER-PROFILE-001` | `pending` | 同名档案得到不同 `ProfileId`、ULID 形态由 `is_valid` 断言（`cargo test -p mien-core --lib storage::sqlite`）；缺“非法标识被拒绝”的用例（`profile_dir`/`trash_dir` 会返回 `ValidationFailed`，但无测试），且 `profiles.name` 无唯一约束，同名依赖复制路径而非约束拒绝。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-002` | `pending` | create/update/archive/restore/soft_delete/purge/copy 的成功路径与 `ProxyNotFound`、`ProfileDeleted` 等失败码均有断言；缺“不可变字段与未知字段”的请求级测试（`ProfilePatch`/`CreateProfile` 未启用 `deny_unknown_fields`）。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-003` | `pending` | 列表按 id 游标分页、`limit` 夹在 1..=100（`--lib storage::sqlite` 的 `list_paginates_by_id_cursor`）；缺 100 档案量级、按名称/分组/标签/归档/运行状态筛选与“页大小上限被拒绝”的断言（当前是静默夹取而非报错）。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-004` | `pending` | `cargo test -p mien-core --test fingerprint_consistency`（9 passed）与 `MIEN_KERNEL_BIN="$KERNEL" cargo test -p mien-core --test slice_e2e` 覆盖两种 `copy_fingerprint`、新 id、新种子、显式字段一致与派生特征重置；缺“新数据目录不同”与“源档案不受影响”的直接断言。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-005` | `pending` | 两个档案的 `downloads` 目录互相独立（`--lib storage::sqlite` 的 `creates_and_copies_isolated_profiles`）；缺 Chromium 默认下载目录绑定、Cookie/缓存/LocalStorage 隔离与 0700 权限断言。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-006` | `pending` | 归档→恢复→软删除→`trash/<id>`→恢复→清理全链路由 `lifecycle_moves_and_restores_directory` 断言，`ProfileDeleted` 与 `ProfileRestoreUnavailable` 有实现；缺“运行中删除返回 `RUNTIME_CONFLICT`”（运行时不落库）、移动失败/提交失败回滚与 `deletion_error` 重试的注入用例。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-007` | `pending` | 迁移幂等与完整性由 `migration_runner_is_idempotent_and_database_is_consistent` 覆盖；缺“标签写入失败完全回滚”的注入用例（标签写入本身未实现）。fixture 根 `fixtures/profile/`、`fixtures/db/` |
| `VER-PROFILE-008` | `pending` | `groups`/`tags` 表有 `UNIQUE` 约束；没有分组/标签仓储方法，重复名拒绝与重复关联幂等均无实现与测试。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-009` | `pass` | `cargo test -p mien-server --test export_import`：`validate` 前后档案行数、`profiles/` 条目与 `runtimes/imports/` 清单数完全相等；整批成功才返回 `imported`；`abort` 冲突零写入、`new_id` 分配新 ULID 并保留 `import_source_id`；第二项写入失败时首行回滚且本请求目录清理、清单删除；启动对账只删无档案行引用的目录（`startup_reconciliation_removes_only_uncommitted_import_directories`）。`validate_relative_path` 仍覆盖 `../`、绝对路径与符号链接（`--lib storage::sqlite` 的 `rejects_unsafe_paths`）。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-010` | `pass` | `cargo test -p mien-server --test export_import`：明文载荷通过独立的 `profile-export.schema.json` 断言且无凭据值/Token/Cookie/顶层 `window`；加密载荷无口令不可读、正确口令可还原（指纹逐字段相等）、错误口令与篡改容器只报 `passphrase` 字段；口令不出现在 URL、日志、审计、事件与错误消息；代理只含 `credential_fields` 与非秘密 `options`、`credentials_removed=true`。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-011` | `pending` | 代理凭据在库中以密文存储且不复现于视图（`cargo test -p mien-core --test proxy_storage`）；缺档案响应、日志、审计与错误消息的扫描。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-012` | `pending` | `rejects_unsafe_paths` 覆盖 `../`、绝对路径与符号链接（`--lib storage::sqlite`）；缺导入路径与 0700 权限断言。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-013` | `pending` | 时间字段由 `now_utc` 写入且为 UTC ISO-8601 毫秒精度；缺“按操作更新”“恢复后保留 `created_at`”的逐字段断言。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-014` | `pending` | 顶层 `window` 确实不存在于 `CreateProfile`/`ProfilePatch`/`Profile`/迁移，窗口只经 `fingerprint.window` 进入 argv（`--test fingerprint_consistency`、`--test mode_matrix`）；缺“拒绝顶层 `window`”的显式负例，`CreateProfile` 未启用 `deny_unknown_fields`。fixture 根 `fixtures/profile/` |
| `VER-PROFILE-015` | `pending` | 每档案内核迁移（预检/审计/回滚）未实现。fixture 根 `fixtures/profile/` |

## M1 slice evidence

- Delivered: `profiles` migrations (0002/0003), `SqliteProfileRepository` (create/get/list/update/archive/restore/soft delete/purge/copy), isolated `profiles/<id>` and `trash/<id>` directories, path-traversal/absolute-path/symlink rejection, and `proxy_id` binding (`PROXY_NOT_FOUND` on a missing proxy row).
- Commands: `cargo test -p mien-core` (storage and slice tests), `cargo run -q -p xtask -- check`.
- Not delivered yet: import/export endpoints and payload schema wiring, groups/tags API surface, download-directory UI binding. Their VER rows stay `pending`.

## M2 slice evidence

- Re-run in T19 on 2026-11-27, macOS 27.2 / arm64:
  - `cargo test -p mien-core --lib storage::sqlite` — 6 passed（创建/复制隔离、两种复制模式、迁移幂等、生命周期与 `trash`、路径安全）。
  - `cargo test -p mien-core --test fingerprint_consistency` — 9 passed（复制重抽种子与派生特征重置）。
  - `cargo test -p mien-core --test proxy_storage` — 11 passed（凭据密文与视图脱敏）。
  - `cargo test --manifest-path src-tauri/Cargo.toml --locked` — 1 + 1 + 7 + 5 passed（IPC 层的 create/list/update/runtime 命令）。
- No profile VER row states a full pass condition yet; the strongest partials are 004, 006, 012, 014.

## Related Changes

- None

## References

- [`./SPEC.md`](./SPEC.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../DATABASE-SCHEMA.md`](../../DATABASE-SCHEMA.md)
- [`../../ERROR-CODES.md`](../../ERROR-CODES.md)

最近同步：M2（T19），依据 `cargo test -p mien-core --lib storage::sqlite`、`cargo test -p mien-core --test fingerprint_consistency`、`cargo test --manifest-path src-tauri/Cargo.toml --locked`、`cargo run -q -p xtask -- check --skip-host`。
