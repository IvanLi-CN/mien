# 环境档案管理

> 本文件是档案主题的长期需求契约。实现覆盖与交付事实写入同目录 `IMPLEMENTATION.md`，生命周期与兼容性背景写入 `HISTORY.md`。

## Context and Scope

- Context: 产品需要在同一台机器或同一个服务端上维护多个彼此隔离的浏览器环境，并为每个环境保存可复现的指纹规格、代理配置和运行偏好。
- In scope: 档案标识、增删改查、复制、归档、回收站与永久删除、分组、标签、搜索、导入导出、数据目录与下载隔离、路径安全、事务与迁移、敏感字段脱敏。
- Out of scope: 指纹字段语义与校验规则（由 `fingerprint-engine` 主题负责）、运行时启动与进程监督（由 `browser-runtime` 负责）、代理连接行为（由 `proxy-gateway` 负责）。

## Terms and Interfaces

- `profile`: 一个可独立启动的浏览器环境，拥有唯一 ULID、独立数据目录（`chrome`、`downloads`、`logs`）、指纹规格、可选代理和运行偏好。
- `archive`: 归档状态，不是回收站软删除；档案不再出现在默认列表，数据目录原地保留，可通过 restore 回到 `active`。
- `deleted`: 回收站状态；数据目录移动到 `<data-root>/trash/<profile-id>` 并写入 `deleted_at`；档案不可启动，可 restore、purge 或只读读取。
- `purge`: `DELETE` 携带 `permanent=true` 的永久删除；先删除数据目录再删除数据库记录，失败时保留 `deleted` 状态并写入 `deletion_error` 供重试。
- `profile status`: `active`、`archived` 或 `deleted`；与运行时 `state` 分离。
- `data root`: macOS 的 `~/Library/Application Support/Mien` 或 Docker 的 `/data`。
- `isolation`: 运行时隔离结论，取值 `enforced`、`unsandboxed` 或 `unknown`；持久化于运行时记录并随档案与运行时响应返回。
- Interface: 控制面 `/api/v1/profiles`、`/api/v1/groups`、`/api/v1/tags` 系列端点，以及 `POST /api/v1/profiles/import/validate`、`POST /api/v1/profiles/import` 和 `POST /api/v1/profiles/{id}/export`；SQLite `profiles`、`groups`、`tags`、`profile_tags` 表；导入导出 JSON schema。
- 导入导出 schema：明文导出结构由 [`profile-export.schema.json`](../../contracts/profile-export.schema.json) 定义，版本独立于应用版本，必须包含 `schema_version`、`exported_at`、`profiles`；`groups`、`tags` 为可选；payload 内的 `schema_version`/`exported_at` 是权威副本，envelope 顶层同名字段必须与之一致。

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

- The system MUST treat duplicate as a new profile. Duplication MUST generate a new fingerprint seed and a new browser data directory; no copy option may reuse the source seed.
- Inputs: 源档案标识、可选的配置复制选项 `copy_fingerprint` 与 `copy_proxy`。
- Outputs: 新档案标识；不得共享运行时、数据目录、下载目录、种子或 Cookie。
- Defaults: `copy_fingerprint=false`、`copy_proxy=true`。
- Copy semantics: `copy_fingerprint=true` 时复制指纹规格的显式字段（`platform`、`platform_version`、`brand`、`brand_version`、`hardware_concurrency`、`languages`、`timezone`、`window`、`spoof`、`webrtc`、`network`），但 MUST 始终生成新的 `seed`；不得复用源档案种子。生成的新种子若与任何现有档案相同必须内部重抽，不返回用户可见的冲突错误。`copy_proxy=true` 时以源 `proxies` 行为模板新建一条 `proxies` 行，用新 `proxy_id` 作为 AES-GCM AAD 重新加密凭据，并让新档案的 `proxy_id` 指向新行；不复用源 `proxy_id`，因此凭据轮换与删除互不影响。复制协议、服务器、端口与非秘密选项，不复制运行状态、健康检查结果或连接池；`copy_proxy=false` 时新档案没有代理。

### REQ-PROFILE-005

- The system MUST isolate every profile in its own browser data directory and MUST NOT reuse a directory for another profile.
- Inputs: 档案标识与数据根路径。
- Outputs: `<data-root>/profiles/<profile-id>/chrome`、`<data-root>/profiles/<profile-id>/downloads` 及独立日志目录；`downloads` MUST 作为该档案 Chromium 的默认下载目录；路径必须保持在档案目录内，子目录权限为 0700；不同档案不得共享 Cookie、缓存、LocalStorage、IndexedDB、Service Worker 或已下载文件。

### REQ-PROFILE-006

- The system MUST implement the profile lifecycle as `active <-> archived`、`active|archived -> deleted`、`deleted -> active` 与 `deleted -> purge`，并 MUST 拒绝启动 `deleted` 档案。
- Transitions: `POST /api/v1/profiles/{id}/archive` 把 `active` 转为 `archived` 并写 `archived_at`；`POST /api/v1/profiles/{id}/restore` 把 `archived` 或 `deleted` 转为 `active`，清空 `archived_at` 与 `deleted_at`；`DELETE /api/v1/profiles/{id}?permanent=false` 把 `active|archived` 转为 `deleted`，把数据目录移动到 `<data-root>/trash/<profile-id>` 并写 `deleted_at`，对已 `deleted` 的档案幂等返回当前状态；`DELETE /api/v1/profiles/{id}?permanent=true` 仅接受 `deleted` 档案，先删除 `<data-root>/trash/<profile-id>`、再删除数据库记录；`trash/<profile-id>` 已缺失时视为删除完成，`profiles/<profile-id>` 缺失不构成删除完成的证据；对 `active|archived` 返回 `PROFILE_NOT_DELETED`（409）。
- Inputs: 档案标识、`permanent` 删除模式、确认令牌。
- Outputs: 状态与目录移动同步；软删除或 purge 前必须确认运行时已停止并完成 teardown，运行中档案一律返回 `RUNTIME_CONFLICT`，不得隐式停止或移动/删除其目录。软删除目录移动失败时行保持原状态、目录不动、`deleted_at` 保持 NULL，写入可重试的 `deletion_error` 并返回 `PROFILE_PURGE_FAILED`（500）；移动成功后才在事务中写 `deleted_at`，事务提交失败时把目录移回原位置。从回收站恢复前必须确认 `<data-root>/trash/<profile-id>` 仍是目录；缺失时返回 `PROFILE_RESTORE_UNAVAILABLE`（409），不得创建空目录，行保持 `deleted`。恢复成功时目录移回 `<data-root>/profiles/<profile-id>` 并清空 `deleted_at` 与 `deletion_error`；`deleted` 档案的启动请求返回 `PROFILE_DELETED`（409）且不得隐式恢复，但 restore、purge、读取与 purge 重试仍可用；purge 目录删除失败时保留 `deleted` 状态、不删除记录、写入并返回稳定 `deletion_error`，修复后可重试 purge；目录已缺失时 purge 视为目录删除已完成并继续删除行，行删除失败才保留记录。

### REQ-PROFILE-007

- The system MUST persist profile metadata, group membership, and tags transactionally in SQLite with foreign keys and explicit migrations.
- Inputs: 档案变更、分组变更、标签变更、schema 迁移。
- Outputs: 全部成功或全部回滚；迁移失败不得留下半迁移状态。

### REQ-PROFILE-008

- The system MUST enforce unique group names and unique tag names, while allowing a profile to belong to multiple tags.
- Inputs: 分组名称、标签名称、档案标签集合。
- Outputs: 冲突时返回稳定错误码；非冲突时返回规范化后的集合。
- Error mapping: 分组返回 `GROUP_NAME_CONFLICT`，标签返回 `TAG_NAME_CONFLICT`。

代理配置是独立的全局资源，不由 `profiles` 外键拥有；复制时新建的代理行也遵循同一规则。purge 在删除目录后以单个事务删除该 `profiles` 行，并由外键级联删除其 `profile_tags` 与终态 `runtimes` 行；保留共享的 `groups`、`tags`、所有 `proxies`、内核/引擎安装和历史审计。界面必须提示代理凭据不会随档案 purge 删除，并提供代理管理入口；操作者可显式删除无档案引用的代理配置，仍有引用时返回 `RESOURCE_IN_USE`。不得以 purge 顺带删除共享资源，也不得声称代理秘密已销毁。

### REQ-PROFILE-009

- The system MUST validate import input against the declared export schema before writing anything, and MUST expose a dry-run that creates no rows or directories.
- Inputs: `ProfileImportInput` = `{format, payload, passphrase?, conflict_strategy?}`；`format` 为 `plain|encrypted`；`payload` 为明文导出对象或不透明加密容器；`passphrase` 为 write-only，`format=encrypted` 时必填、`format=plain` 时禁止；`conflict_strategy` 为 `new_id|abort`，默认 `new_id`。
- Outputs: `import/validate` 返回逐项草稿、`errors` 与冲突，不写行、不建目录；`import` 在单个事务内提交。目录创建前先将请求拥有的新 ULID 与相对目录清单持久化到 `<data-root>/runtimes/imports/<request-id>.json`（0600），不记录配置、口令或凭据。写入失败先回滚行，再只清理该清单中的新目录，不复建已回滚的软删除行；输入/冲突错误分别为 422 `VALIDATION_FAILED`/409 `PROFILE_IMPORT_CONFLICT`，存储失败按错误码目录返回。目录清理失败保留清单并返回可重试 500 `PROFILE_IMPORT_CLEANUP_FAILED`，由启动对账与后台清理重试；清理必须规范化路径、拒绝符号链接，并先确认目录 ID 不对应已提交档案，不能删除后来已提交或其他请求的目录。提交成功但清单未删除时，对账保留已提交目录并移除清单。整批成功才返回 `ProfileImportResult`，每项含 `profile`、`status: imported` 和 `errors: []`。
- Conflict identity: 档案冲突以导出项的 `source_profile_id` 与目标库 `profiles.import_source_id`（或现有 `profiles.id`）匹配；分组与标签以名称匹配。导出必须携带稳定的 `source_profile_id`，导入不得复用该值作为新档案 ID。
- Conflict semantics: `new_id` 为冲突项分配新 ULID、保留 `import_source_id` 并保留配置；`abort` 在写入前返回 `PROFILE_IMPORT_CONFLICT`，不产生任何档案、目录、分组或标签。
- Validation: 非法 schema、未知 `schema_version`、超大文件、字段超限、代理配置形状、分享链接字段、`../` 与绝对路径、符号链接必须被拒绝；加密导出未提供口令或口令错误时返回字段级错误且不回显口令。
- Transport: Docker Web UI 先调用 validate，再调用 import 提交；桌面宿主使用同语义的 IPC 命令。两者在确认与成功结果中均提示“导入保留原 seed；使用相同内核与规格时指纹相同”，不等待本机找到相同 seed 才显示该提示。

### REQ-PROFILE-010

- The system MUST exclude proxy credentials, API tokens, cookies, and browser data from default exports, and MUST support passphrase-encrypted exports using authenticated encryption.
- Inputs: `POST /api/v1/profiles/{id}/export` 的请求体 `ProfileExportRequest` = `{format, passphrase?}`；`format` 为 `plain|encrypted`；`passphrase` 为 write-only，`format=encrypted` 时必填、`format=plain` 时禁止；口令只允许出现在请求体，禁止出现在 URL、查询串、日志、审计和错误消息中。
- Outputs: `ProfileExportEnvelope`；明文 `payload` 遵循 [`profile-export.schema.json`](../../contracts/profile-export.schema.json) 的结构；加密 `payload` 是带版本号的不透明 `profile-encrypted-v1` 字符串容器，Argon2id 参数、salt 与认证密文都位于该不透明值内部，不出现在 envelope 顶层；envelope 版本参与 AAD，错误口令返回字段级错误。
- Redaction: 导出中的代理只保留 `kind`、`server`、`port`、非秘密 `options` 和 `credential_fields` 名称列表，并固定 `credentials_removed=true`；`options` MUST 拒绝密码、UUID、私钥、Token、Cookie 等秘密键。
- Canonical fields: 窗口尺寸只存在于指纹规格中；导出不含顶层 `runtime.window`，也不含 `window_json`。

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

- The system MUST define startup preferences per profile, including start URL, headful/headless/pseudo_headful mode, and whether the automation API is exposed; window size MUST NOT be duplicated at profile level.
- Inputs: 用户偏好与能力清单。
- Outputs: 持久化偏好；`ProfileInput`、`ProfilePatch` 与 `Profile` 不含顶层 `window`，窗口尺寸只从 `fingerprint.window` 读取；不支持的显示或运行模式在保存时拒绝或在启动时给出明确错误。

### REQ-PROFILE-015

- The system MUST let each profile bind an installed kernel version and MUST implement kernel migration as an explicit, audited, reversible operation.
- Inputs: 目标 `kernel_version`、当前 `kernel_version`、已安装内核记录（含 `asset_sha256` 与 `binary_sha256`）、指纹规格、沙箱能力。
- Outputs: 预检平台、能力清单（含 `window-size` 与 `per_platform_values`）、指纹规格一致性和沙箱可用性；运行中的档案返回 `RUNTIME_CONFLICT`（409），不得隐式停止；仅已停止的档案可迁移；预检通过后写入新 `kernel_version`，并记录前后版本、预检结果和执行者的审计事件。
- Failure semantics: 预检或迁移失败时保留原内核绑定，不产生部分迁移的档案；迁移后重新校验指纹规格并提示派生特征变化；保留上一版本用于回滚，回滚同样写审计事件。

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

- Method: 复制含种子、代理、运行偏好和指纹规格的档案，分别以 `copy_fingerprint=false` 与 `copy_fingerprint=true` 比较新旧档案。
- covers: `REQ-PROFILE-004`
- Pass condition: 新档案标识与数据目录不同；`copy_fingerprint=false` 时不复制指纹；`copy_fingerprint=true` 时显式字段一致但 `seed` 必然不同；强制注入相同随机值时应内部重抽，不返回种子冲突；源档案运行状态与数据目录不受影响。

### VER-PROFILE-005

- Method: 创建两个档案并启动，在各自页面触发下载，检查 Cookie、缓存、LocalStorage 与下载文件。
- covers: `REQ-PROFILE-005`
- Pass condition: 两个目录互不包含对方数据或下载文件；Chromium 默认下载目录分别为各自的 `<profile-id>/downloads`；数据目录与下载目录权限为 0700。

### VER-PROFILE-006

- Method: 依次执行归档、恢复、删除到回收站、从回收站恢复、彻底删除，并在每个阶段尝试启动；单独模拟目录删除失败后重试。
- covers: `REQ-PROFILE-006`
- Pass condition: 归档与 `deleted` 档案启动均被拒绝且不隐式恢复，`deleted` 档案返回 `PROFILE_DELETED`；运行中档案删除返回 `RUNTIME_CONFLICT` 且不移动目录；`permanent=false` 后目录位于 `<data-root>/trash/<id>` 且 `deleted_at` 已写；恢复后目录回位、`archived_at` 与 `deleted_at` 均清空且可启动；目录缺失时 restore 返回 `PROFILE_RESTORE_UNAVAILABLE` 且不创建空目录；`permanent=true` 后记录与 `trash/<id>` 均消失；模拟软删除目录移动失败时行保持原状态、`deleted_at IS NULL`、`deletion_error` 已写且目录仍在 `profiles/<id>`；模拟移动后事务提交失败时目录被移回 `profiles/<id>`；模拟 `trash/<id>` 删除失败时档案保持 `deleted`、响应包含 `deletion_error`，修复后重试成功；`trash/<id>` 已缺失时 purge 仍可删除行。

### VER-PROFILE-007

- Method: 注入标签写入失败和迁移中断，检查事务结果。
- covers: `REQ-PROFILE-007`
- Pass condition: 失败写入完全回滚；迁移中断后数据库仍可打开且迁移可重入。

### VER-PROFILE-008

- Method: 创建重复分组名、重复标签名和重复标签关联。
- covers: `REQ-PROFILE-008`
- Pass condition: 重复名称被拒绝；重复关联被幂等处理；合法多标签关联保留。

### VER-PROFILE-009

- Method: 分别调用 `POST /api/v1/profiles/import/validate` 与 `POST /api/v1/profiles/import`，使用合法、错误 schema、未知版本、超长字段、`../` 路径、绝对路径、符号链接、加密导出未提供口令和冲突档案。
- covers: `REQ-PROFILE-009`
- Pass condition: validate 零写入；整批成功才返回 `imported`；`abort` 冲突零写入，`new_id` 分配新 ID 并保留来源。最后一项写入失败时数据库零残留，仅清理本请求目录；清理失败返回 `PROFILE_IMPORT_CLEANUP_FAILED` 并保留无秘密的清单，重启可重试且不复建档案行、不删已提交目录或其他请求目录。输入/冲突/存储错误按目录映射而非全部伪装为 422/409；确认与结果都显示保留 seed 的指纹相同提示。

### VER-PROFILE-010

- Method: 调用 `POST /api/v1/profiles/{id}/export`，分别导出 `plain` 与 `encrypted`，检查请求体、响应、日志、审计与解密结果。
- covers: `REQ-PROFILE-010`
- Pass condition: 明文 `payload` 通过 `profile-export.schema.json` 校验且不含代理凭据、Token、Cookie 或 `runtime.window`；加密 payload 在无口令时不可读、正确口令可还原、错误口令返回字段级错误；`passphrase` 不出现在 URL、日志、审计或错误消息；代理仅含 `credential_fields` 名称与非秘密 `options`，`credentials_removed=true`，秘密键被拒绝。

### VER-PROFILE-011

- Method: 扫描 API 响应、日志、审计事件和错误消息中的敏感值。
- covers: `REQ-PROFILE-011`
- Pass condition: 代理凭据、完整代理 URL/分享链接、Cookie 和 Token 均不出现；UI 只显示“已设置”。

### VER-PROFILE-012

- Method: 使用路径穿越、绝对路径和符号链接构造档案目录、下载目录与导入路径。
- covers: `REQ-PROFILE-012`
- Pass condition: 所有越界路径被拒绝；档案目录、`downloads`、日志目录与导入路径均限制在数据根内，正常目录权限为 0700；不跟随符号链接写入。

### VER-PROFILE-013

- Method: 创建、更新、归档、恢复和删除档案，读取时间字段。
- covers: `REQ-PROFILE-013`
- Pass condition: 时间字段按操作更新，格式为 UTC ISO-8601，归档后恢复不丢失创建时间。

### VER-PROFILE-014

- Method: 保存有头/无头/伪有头、启动地址和自动化开关，检查档案响应、导出与启动 argv 中窗口尺寸的来源，并与能力清单比较。
- covers: `REQ-PROFILE-014`
- Pass condition: 支持的偏好被持久化；`ProfileInput`、`ProfilePatch` 与 `Profile` 拒绝顶层 `window`；窗口尺寸只从 `fingerprint.window` 读取并在启动 argv 中生效；不支持的模式在保存或启动时被明确拒绝，不静默降级。

### VER-PROFILE-015

- Method: 为运行中与已停止的档案请求内核迁移，并注入能力清单缺失、指纹不一致、沙箱不可用和写入失败。
- covers: `REQ-PROFILE-015`
- Pass condition: 运行中档案被拒绝且绑定不变；预检失败保留原版本且无部分迁移；成功迁移写入含前后版本、预检结果和执行者的审计事件，并重新校验指纹规格；回滚恢复原版本并写审计。

## Related ADRs

- [ADR-0001：共享 Rust 核心与双宿主](../../adr/0001-shared-core-two-hosts.md)
- [ADR-0002：固定 fingerprint-chromium 148 并建立内核准入](../../adr/0002-pin-kernel-148.md)
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
