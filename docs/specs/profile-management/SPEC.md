# 环境档案管理

> 本文件是档案主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: 产品需要在同一台机器或同一个服务端上维护多个彼此隔离的浏览器环境，并为每个环境保存可复现的指纹规格、代理配置和运行偏好。
- In scope: 档案标识、增删改查、复制、归档、分组、标签、搜索、导入导出、数据目录隔离、路径安全、事务与迁移、敏感字段脱敏。
- Out of scope: 指纹字段语义与校验规则（由 `fingerprint-engine` 主题负责）、运行时启动与进程监督（由 `browser-runtime` 负责）、代理连接行为（由 `proxy-gateway` 负责）。

## Terms and Interfaces

- `profile`: 一个可独立启动的浏览器环境，拥有唯一 ULID、独立数据目录、指纹规格、可选代理和运行偏好。
- `archive`: 软删除状态；档案不再出现在默认列表，但数据在保留窗口内可恢复。
- `profile status`: `active`、`archived` 或 `deleted`；与运行时 `state` 分离。
- `hard delete`: 永久删除元数据和浏览器数据目录；不可恢复。
- `data root`: macOS 的 `~/Library/Application Support/Mien` 或 Docker 的 `/data`。
- Interface: 控制面 `/api/v1/profiles`、`/api/v1/groups`、`/api/v1/tags` 系列端点，以及 `/api/v1/profiles/import` 和 `/api/v1/profiles/{id}/export`；SQLite `profiles`、`groups`、`tags`、`profile_tags` 表；导入导出 JSON schema。
- 导入导出 schema 版本独立于应用版本，至少包含 `schema_version`、`profiles`、`groups`、`tags`。

## Requirements

### REQ-PROFILE-001

- The system MUST assign every profile an immutable ULID at creation and MUST allow human-readable names to repeat.
- Inputs: 名称（最多 120 字符）、可选分组、可选标签（每个最多 64 字符）、初始指纹规格、可选代理、可选启动地址。
- Outputs: 持久化档案记录与唯一 `profile_id`；名称重复不构成错误，标识缺失或非法构成错误。

### REQ-PROFILE-002

- The system MUST support create, read, update, duplicate, archive, restore, and permanent delete operations with stable semantics across desktop and server hosts.
- Inputs: 档案标识、补丁字段、操作者上下文。
- Outputs: 更新后的档案或明确错误；未知字段、非法类型和不可变字段修改必须被拒绝。

### REQ-PROFILE-003

- The system MUST provide paginated listing with filters for name, group, tags, archived state, and runtime state.
- Inputs: 页码或游标、每页大小、筛选条件、排序字段。
- Outputs: 稳定排序的结果页和总数或下一页游标；每页大小必须受上限约束。

### REQ-PROFILE-004

- The system MUST treat duplicate as a new profile. Unless explicitly requested, duplication MUST generate a new fingerprint seed and a new browser data directory.
- Inputs: 源档案标识、可选的配置复制选项。
- Outputs: 新档案标识；不得共享运行时、数据目录、种子或 Cookie。
- Defaults: `copy_fingerprint=false`；显式请求复制指纹时仍必须检查种子冲突并返回 `FINGERPRINT_SEED_CONFLICT`。

### REQ-PROFILE-005

- The system MUST isolate every profile in its own browser data directory and MUST NOT reuse a directory for another profile.
- Inputs: 档案标识与数据根路径。
- Outputs: `<data-root>/profiles/<profile-id>/chrome` 及独立日志目录；不同档案不得共享 Cookie、缓存、LocalStorage、IndexedDB 或 Service Worker。

### REQ-PROFILE-006

- The system MUST resolve archive, restore, and permanent delete without allowing a deleted profile to be started.
- Inputs: 档案标识、删除模式、确认令牌。
- Outputs: 归档档案从默认列表隐藏；恢复后重新可见；永久删除在确认后移除数据库记录和数据目录；删除失败必须回滚状态或标记可重试错误。

### REQ-PROFILE-007

- The system MUST persist profile metadata, group membership, and tags transactionally in SQLite with foreign keys and explicit migrations.
- Inputs: 档案变更、分组变更、标签变更、schema 迁移。
- Outputs: 全部成功或全部回滚；迁移失败不得留下半迁移状态。

### REQ-PROFILE-008

- The system MUST enforce unique group names and unique tag names, while allowing a profile to belong to multiple tags.
- Inputs: 分组名称、标签名称、档案标签集合。
- Outputs: 冲突时返回稳定错误码；非冲突时返回规范化后的集合。
- Error mapping: 分组返回 `GROUP_NAME_CONFLICT`，标签返回 `TAG_NAME_CONFLICT`。

### REQ-PROFILE-009

- The system MUST validate import files against the declared schema, including field types, size limits, proxy configuration shape, share-link fields, and path fields.
- Inputs: JSON 导入文件、可选口令。
- Outputs: 通过校验的档案草稿或逐项错误；非法 schema、超大文件、未知版本、路径穿越和符号链接必须被拒绝。
- Transport: Docker Web UI 使用 `POST /api/v1/profiles/import`；冲突项在用户确认后分配新 ID，否则返回 `PROFILE_IMPORT_CONFLICT`。

### REQ-PROFILE-010

- The system MUST exclude proxy credentials, API tokens, cookies, and browser data from default exports, and MUST support passphrase-encrypted exports using authenticated encryption.
- Inputs: 档案集合、导出模式、可选口令。
- Outputs: 带 schema 版本的导出文件；默认导出不含秘密；加密导出只包含口令派生密钥可解开的密文。
- Transport: Docker Web UI 使用 `GET /api/v1/profiles/{id}/export?format=plain|encrypted`；加密 payload 是不透明容器。

### REQ-PROFILE-011

- The system MUST redact sensitive values from API responses, logs, audit metadata, and error messages.
- Inputs: 档案、代理、Token 和运行时数据。
- Outputs: 代理凭据（密码、UUID、私钥）、完整代理 URL/分享链接、Cookie、Token 和授权头不出现在任何可读输出中；只允许返回“已设置/未设置”。

### REQ-PROFILE-012

- The system MUST reject paths that escape the data root through traversal, absolute paths, or symbolic links, and MUST create profile directories with owner-only permissions.
- Inputs: 档案标识、导入路径、数据根路径、文件系统状态。
- Outputs: 规范化后的路径必须位于数据根内；越界路径返回错误；目录权限为 0700。

### REQ-PROFILE-013

- The system MUST record creation, update, archive, restore, and delete timestamps and MUST expose them in profile details.
- Inputs: 档案写操作。
- Outputs: `created_at`、`updated_at`、`archived_at`、`deleted_at` 按语义更新；时间使用 UTC ISO-8601。

### REQ-PROFILE-014

- The system MUST define startup preferences per profile, including start URL, headful/headless/pseudo_headful mode, window size, and whether the automation API is exposed.
- Inputs: 用户偏好与能力清单。
- Outputs: 持久化偏好；不支持的显示或运行模式在保存时拒绝或在启动时给出明确错误。

## Verification

### VER-PROFILE-001

- Method: API 与数据库集成测试创建同名档案，读取其标识和列表。
- covers: `REQ-PROFILE-001`
- Pass condition: 两个档案名称相同但 `profile_id` 不同，且标识符合 ULID 格式；非法标识被拒绝。

### VER-PROFILE-002

- Method: 对每个操作执行成功与失败路径测试，包括不可变字段和未知字段。
- covers: `REQ-PROFILE-002`
- Pass condition: 成功路径返回预期状态；失败路径返回稳定错误码且数据库不变。

### VER-PROFILE-003

- Method: 生成 100 个档案，按名称、分组、标签、归档状态和运行状态筛选并分页。
- covers: `REQ-PROFILE-003`
- Pass condition: 排序稳定、分页无重复或遗漏，超出上限的页大小被拒绝。

### VER-PROFILE-004

- Method: 复制含种子、代理和运行偏好的档案，比较新档案与源档案。
- covers: `REQ-PROFILE-004`
- Pass condition: 新档案标识、种子和数据目录均不同；配置按选项复制；源档案运行状态不受影响。

### VER-PROFILE-005

- Method: 创建两个档案并启动，检查各自数据目录中的 Cookie、缓存和 LocalStorage 文件。
- covers: `REQ-PROFILE-005`
- Pass condition: 两个目录互不包含对方数据；路径分别指向各自的 `<profile-id>/chrome`。

### VER-PROFILE-006

- Method: 归档、恢复、永久删除，并在每个阶段尝试启动。
- covers: `REQ-PROFILE-006`
- Pass condition: 归档档案启动被拒绝；恢复后可启动；永久删除后记录与目录消失；模拟删除失败时状态可重试。

### VER-PROFILE-007

- Method: 注入标签写入失败和迁移中断，检查事务结果。
- covers: `REQ-PROFILE-007`
- Pass condition: 失败写入完全回滚；迁移中断后数据库仍可打开且迁移可重入。

### VER-PROFILE-008

- Method: 创建重复分组名、重复标签名和重复标签关联。
- covers: `REQ-PROFILE-008`
- Pass condition: 重复名称被拒绝；重复关联被幂等处理；合法多标签关联保留。

### VER-PROFILE-009

- Method: 使用合法、错误 schema、超长字段、`../` 路径、绝对路径和符号链接导入文件。
- covers: `REQ-PROFILE-009`
- Pass condition: 合法文件导入成功；其余输入被拒绝并返回具体字段错误；不产生越界文件。

### VER-PROFILE-010

- Method: 执行默认导出与加密导出，检查文件内容和解密结果。
- covers: `REQ-PROFILE-010`
- Pass condition: 默认导出不含代理凭据、Token、Cookie；加密导出无法在无口令时读取，正确口令可还原配置。

### VER-PROFILE-011

- Method: 扫描 API 响应、日志、审计事件和错误消息中的敏感值。
- covers: `REQ-PROFILE-011`
- Pass condition: 代理凭据、完整代理 URL/分享链接、Cookie 和 Token 均不出现；UI 只显示“已设置”。

### VER-PROFILE-012

- Method: 使用路径穿越、绝对路径和符号链接构造档案目录与导入路径。
- covers: `REQ-PROFILE-012`
- Pass condition: 所有越界路径被拒绝；正常目录权限为 0700；不跟随符号链接写入。

### VER-PROFILE-013

- Method: 创建、更新、归档、恢复和删除档案，读取时间字段。
- covers: `REQ-PROFILE-013`
- Pass condition: 时间字段按操作更新，格式为 UTC ISO-8601，归档后恢复不丢失创建时间。

### VER-PROFILE-014

- Method: 保存有头/无头/伪有头、窗口尺寸、启动地址和自动化开关，并与能力清单比较。
- covers: `REQ-PROFILE-014`
- Pass condition: 支持的偏好被持久化；不支持的模式在保存或启动时被明确拒绝，不静默降级。

## Related ADRs

- [ADR-0001：共享 Rust 核心与双宿主](../../adr/0001-shared-core-two-hosts.md)
- [ADR-0003：SQLite 存储与字段级密钥保护](../../adr/0003-storage-and-secret-protection.md)
- [ADR-0006：显式指纹规格与种子派生模型](../../adr/0006-fingerprint-spec-model.md)

## Visual Evidence

- None

## References

- [`./IMPLEMENTATION.md`](./IMPLEMENTATION.md)
- [`./HISTORY.md`](./HISTORY.md)
- [`../../PRD.md`](../../PRD.md)
- [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md)
- [`../../contracts/profile-export.schema.json`](../../contracts/profile-export.schema.json)
