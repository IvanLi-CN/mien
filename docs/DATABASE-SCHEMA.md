# Mien SQLite 数据库契约

本文固定首个版本的逻辑 schema、约束、索引、迁移和加密边界。它是 `docs/ARCHITECTURE.md` 数据模型的详细化；第 2 节 DDL 必须能在空库直接执行，实现时再用 `sqlx` 迁移或等价机制把它拆成不可变、按版本递增的迁移。

## 1. 全局约定

- SQLite 使用 WAL、外键和 `busy_timeout=5000`。
- 业务主键使用 ULID 文本；单例表（`proxy_engine_state`、`admin_credentials`）与自增事件序列（`event_log.sequence`）除外；时间使用 UTC ISO-8601 文本。
- JSON 字段只保存已经过 schema 校验的非秘密数据。
- 代理凭据、API Token 明文、Cookie、主密钥和私钥不得以明文写入数据库。
- 所有跨表写入必须在一个短事务内完成；不得在持锁时等待进程、网络或共享引擎。
- 连接池为单写者语义；写冲突返回 `STORAGE_BUSY`，由调用方短退避重试。
- 迁移只增不改；任何破坏性变更必须先备份并需要显式确认。
- 第 2 节 DDL 必须能在空库中直接执行；CI 在 SQLite 上执行整份 DDL 并运行 `PRAGMA foreign_key_check` 与 `PRAGMA integrity_check`。

## 2. DDL

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  applied_at TEXT NOT NULL
);

CREATE TABLE groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE tags (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  color TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE proxies (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('http','https','socks5','vless','vmess','shadowsocks','trojan','hysteria2','wireguard')),
  server TEXT NOT NULL,
  port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
  config_ciphertext BLOB NOT NULL,
  credential_fields_json TEXT NOT NULL DEFAULT '{}',
  last_check_json TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE profiles (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  group_id TEXT REFERENCES groups(id) ON DELETE SET NULL,
  notes TEXT,
  fingerprint_json TEXT NOT NULL,
  fingerprint_schema INTEGER NOT NULL,
  kernel_version TEXT NOT NULL,
  proxy_id TEXT REFERENCES proxies(id) ON DELETE SET NULL,
  mode TEXT NOT NULL DEFAULT 'headful' CHECK (mode IN ('headful','headless','pseudo_headful')),
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived','deleted')),
  start_url TEXT,
  automation_api INTEGER NOT NULL DEFAULT 0 CHECK (automation_api IN (0,1)),
  runtime_json TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT,
  deleted_at TEXT,
  deletion_error TEXT,
  CHECK (status <> 'deleted' OR deleted_at IS NOT NULL),
  CHECK (deletion_error IS NULL OR status = 'deleted')
);

CREATE INDEX idx_profiles_group ON profiles(group_id);
CREATE INDEX idx_profiles_status_updated ON profiles(status, updated_at DESC);
CREATE INDEX idx_profiles_name ON profiles(name);

CREATE TABLE profile_tags (
  profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
  tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (profile_id, tag_id)
);

CREATE INDEX idx_profile_tags_tag ON profile_tags(tag_id);

CREATE TABLE kernel_installs (
  version TEXT NOT NULL,
  platform TEXT NOT NULL CHECK (platform IN ('macos-arm64','linux-x86_64','windows-x64')),
  path TEXT NOT NULL,
  asset_sha256 TEXT NOT NULL CHECK (length(asset_sha256) = 64),
  binary_sha256 TEXT NOT NULL CHECK (length(binary_sha256) = 64),
  manifest_json TEXT NOT NULL,
  active INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0,1)),
  installed_at TEXT NOT NULL,
  PRIMARY KEY (version, platform)
);

CREATE UNIQUE INDEX idx_kernel_active_per_platform
  ON kernel_installs(platform) WHERE active = 1;

CREATE TABLE proxy_engine_installs (
  engine TEXT NOT NULL CHECK (engine = 'xray-core'),
  version TEXT NOT NULL,
  platform TEXT NOT NULL CHECK (platform IN ('macos-arm64','linux-x86_64','windows-x64')),
  path TEXT NOT NULL,
  asset_sha256 TEXT NOT NULL CHECK (length(asset_sha256) = 64),
  binary_sha256 TEXT NOT NULL CHECK (length(binary_sha256) = 64),
  manifest_json TEXT NOT NULL,
  license TEXT NOT NULL CHECK (license = 'MPL-2.0'),
  source_url TEXT NOT NULL,
  active INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0,1)),
  installed_at TEXT NOT NULL,
  PRIMARY KEY (engine, version, platform)
);

CREATE UNIQUE INDEX idx_engine_active_per_platform
  ON proxy_engine_installs(platform) WHERE active = 1;

CREATE TABLE proxy_engine_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  pid INTEGER,
  api_port INTEGER,
  state TEXT NOT NULL CHECK (state IN ('starting','running','stopping','stopped','crashed')),
  started_at TEXT,
  last_error TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE proxy_outbounds (
  outbound_key TEXT PRIMARY KEY,
  route_key_json TEXT NOT NULL,
  ref_count INTEGER NOT NULL DEFAULT 0 CHECK (ref_count >= 0),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE runtimes (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL UNIQUE REFERENCES profiles(id) ON DELETE CASCADE,
  state TEXT NOT NULL CHECK (state IN ('starting','running','stopping','stopped','crashed','failed')),
  mode TEXT NOT NULL CHECK (mode IN ('headful','headless','pseudo_headful')),
  isolation TEXT NOT NULL DEFAULT 'unknown' CHECK (isolation IN ('enforced','unsandboxed','unknown')),
  pid INTEGER,
  pgid INTEGER,
  proxy_port INTEGER,
  proxy_inbound_tag TEXT,
  proxy_outbound_key TEXT REFERENCES proxy_outbounds(outbound_key) ON DELETE SET NULL,
  proxy_rule_tag TEXT,
  cdp_port INTEGER,
  started_at TEXT,
  exit_code INTEGER,
  last_error TEXT,
  updated_at TEXT NOT NULL
);

CREATE INDEX idx_runtimes_state ON runtimes(state);
CREATE INDEX idx_runtimes_outbound ON runtimes(proxy_outbound_key);

CREATE TABLE admin_credentials (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  password_hash TEXT NOT NULL,
  algo TEXT NOT NULL CHECK (algo = 'argon2id'),
  params_json TEXT NOT NULL,
  must_change INTEGER NOT NULL DEFAULT 1 CHECK (must_change IN (0,1)),
  bootstrap_pending INTEGER NOT NULL DEFAULT 0 CHECK (bootstrap_pending IN (0,1)),
  updated_at TEXT NOT NULL
);

CREATE TABLE api_tokens (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  scopes_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_used_at TEXT,
  revoked_at TEXT
);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  last_used_at TEXT,
  revoked_at TEXT
);

CREATE INDEX idx_sessions_expires ON sessions(expires_at);

CREATE TABLE event_tickets (
  ticket_hash TEXT PRIMARY KEY,
  session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
  token_id TEXT REFERENCES api_tokens(id) ON DELETE CASCADE,
  expires_at TEXT NOT NULL,
  used_at TEXT,
  created_at TEXT NOT NULL,
  CHECK (session_id IS NOT NULL OR token_id IS NOT NULL)
);

CREATE INDEX idx_event_tickets_expires ON event_tickets(expires_at);

CREATE TABLE event_log (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX idx_event_log_type_sequence ON event_log(type, sequence);

CREATE TABLE idempotency_keys (
  key TEXT NOT NULL,
  scope TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  response_json TEXT,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  PRIMARY KEY (key, scope)
);

CREATE INDEX idx_idempotency_expires ON idempotency_keys(expires_at);

CREATE TABLE audit_events (
  id TEXT PRIMARY KEY,
  actor TEXT NOT NULL,
  event_type TEXT NOT NULL,
  profile_id TEXT,
  action TEXT NOT NULL,
  target TEXT NOT NULL,
  result TEXT NOT NULL CHECK (result IN ('success','failure','denied')),
  request_id TEXT NOT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL
);

CREATE INDEX idx_audit_created ON audit_events(created_at DESC);
CREATE INDEX idx_audit_event_type_created ON audit_events(event_type, created_at DESC);
CREATE INDEX idx_audit_profile_created ON audit_events(profile_id, created_at DESC);
CREATE INDEX idx_audit_action_created ON audit_events(action, created_at DESC);
CREATE INDEX idx_audit_request ON audit_events(request_id);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (1, 'initial schema', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
```

## 3. 约束与事务

- 删除档案默认是软删除：`DELETE ?permanent=false` 把 `profiles.status` 置为 `deleted`、写 `deleted_at`，把档案目录移动到 `<data-root>/trash/<id>`；行与目录在 restore 前保留。
- 永久删除是显式操作：`DELETE ?permanent=true` 先删除 trash 目录，再删除行。目录删除失败时保留 `deleted` 行并写 `deletion_error`，供重试；`deletion_error` 只能是稳定、非敏感的原因标识（例如 `busy`、`permission_denied`），不得包含绝对路径、用户名或原始 OS 消息。
- 档案生命周期：`active <-> archived` 由 archive/restore 驱动，restore 清空 `archived_at`；`active|archived -> deleted` 由软删除驱动；`deleted -> active` 由 restore 驱动、恢复目录并清空 `archived_at`/`deletion_error`；`deleted -> purge` 由永久删除驱动。`deleted_at` 只记录最近一次软删除时间，状态判据始终是 `profiles.status`；`deleted` 档案不能启动，启动请求返回 `PROFILE_DELETED`，运行控制不得隐式恢复档案。
- 窗口尺寸只存在于 `fingerprint_json`；顶层启动偏好只保留 `start_url` 和 `automation_api`，数据库不再有 `window_json`，导出与偏好 API 也不得出现顶层 `window`。
- 下载目录固定为 `<data-root>/profiles/<id>/downloads`（0700），作为 Chromium 默认下载目录；所有路径必须规范化并校验在该档案目录前缀内。
- `runtimes.profile_id` 唯一，保证一个档案最多一个受监督运行时；启动/停止通过事务和档案锁串行化。
- `runtimes.isolation` 保存实际沙箱状态：`enforced`、`unsandboxed` 或 `unknown`；每次启动探测后写入，`unknown` 不得当作安全，proof strip 由该字段派生。
- `proxy_outbounds.ref_count` 只统计非终态运行时（`starting`/`running`/`stopping`）；`stopped`/`crashed`/`failed` 为终态。停止或失败 teardown 必须先清空该运行时的 `proxy_outbound_key`、`proxy_inbound_tag`、`proxy_rule_tag`、`proxy_port`，再递减 `ref_count`；teardown 幂等，引用只释放一次。启动时对账并修复，不一致返回 `PROXY_OUTBOUND_REFCOUNT_MISMATCH`。
- `kernel_installs` 与 `proxy_engine_installs` 同时记录 `asset_sha256`（下载的发布资产摘要）与 `binary_sha256`（解包后可执行文件摘要）。安装时校验发布资产，运行时只校验 `binary_sha256`；文件缺失返回 `KERNEL_NOT_FOUND`/`PROXY_ENGINE_NOT_FOUND`，摘要不符返回 `KERNEL_CHECKSUM_MISMATCH`/`PROXY_ENGINE_CHECKSUM_MISMATCH`。`manifest_json` 保存安装时的候选清单。
- `event_log.sequence` 是单调递增的整数回放游标（WebSocket `after=sequence`），`event_id` 是事件标识；不得用字符串游标代替 `sequence`。
- `event_tickets` 只保存一次性 WebSocket 票据哈希；票据绑定会话或 API Token，短 TTL、单次使用，用过或过期即失效；签发与升级都必须重新检查会话/Token 未撤销（`revoked_at IS NULL`）并校验 Origin 允许列表；会话或 Token 行删除时外键级联删除票据。
- 管理员凭据单行存于 `admin_credentials`。引导密码来自 `MIEN_ADMIN_PASSWORD` 或 `MIEN_ADMIN_PASSWORD_FILE`（0600）；两者都未提供时生成高熵密码写入 `/data/secrets/admin-bootstrap`（0600）。密码以 Argon2id 哈希写入，并置 `must_change=1`、`bootstrap_pending=1`；改密成功后清空这两个标记并删除引导秘密（环境变量、Secret 或 `admin-bootstrap` 文件）。
- `proxies.last_check_json` 保存最近一次 `ProxyCheck`。代理节点网络失败（拒绝连接、认证、TLS、超时、DNS）作为检查结果返回，HTTP 200 且 `ok=false`、`error_code` 稳定；检查失败不得改写为未声明的 502 响应。
- `copy_fingerprint=true` 时复制显式字段但总是生成新 seed；导入保留导出 payload 中的 seed 以复现指纹。seed 不要求全局唯一，不存在种子冲突分支或对应错误码。
- 共享引擎状态是单例；`proxy_engine_state.id=1` 固定。
- 一个平台最多一个 active 内核和一个 active 引擎；切换 active 必须在事务中完成。
- `event_log`、`idempotency_keys`、`sessions` 和审计事件按保留策略清理；清理不得删除当前运行时所需的行。
- 幂等响应只保存非敏感响应体；如果响应包含一次性 Token 或凭据，不写入 `response_json`。`tokensCreate` 即使带 `Idempotency-Key` 也不重放明文 Token，重复键返回 `IDEMPOTENCY_CONFLICT`。
- `groups.name` 和 `tags.name` 唯一；冲突分别返回 `GROUP_NAME_CONFLICT`/`TAG_NAME_CONFLICT`。审计记录必须带 `request_id`，用于与请求和事件关联。
- 审计以 `event_type` 为规范动作类型，`profile_id` 支撑按档案筛选，`action` 只作兼容字段保留；三个筛选维度必须都能走索引。`audit_events.profile_id` 是历史标识、不设外键，档案 purge 后审计记录仍保留原 ID。

## 4. 加密与数据边界

- `proxies.config_ciphertext` 使用 AES-256-GCM 字段级加密；关联数据绑定 `proxy_id` 和字段名。
- `credential_fields_json` 只记录已设置的凭据字段名，不记录值。
- `api_tokens.token_hash` 只保存 `HMAC-SHA-256(pepper, token)`，其中 `pepper = HKDF-SHA256(master_key, info="mien/api-token-pepper/v1")` 不单独落盘；绝不使用 AES-GCM、SQLCipher 或其他可逆存储，比较使用常量时间实现。明文只在创建响应中出现一次，且不得写入幂等缓存、日志或事件。
- 主密钥轮换会改变 pepper，因此轮换后全部 API Token 立即失效并要求重新签发；迁移必须按 ADR-0003 的轮换步骤处理已加密字段。
- `sessions.token_hash` 只保存随机标识哈希；Cookie 值不落库。
- `admin_credentials.password_hash` 使用 Argon2id（随机 salt、参数版本化并存入 `params_json`），与主密钥独立；主密钥轮换不改变口令哈希。引导密码明文不得落库、日志、事件或镜像层。
- `fingerprint_json` 和 `runtime_json` 不含秘密。
- 主密钥、代理凭据、Token 明文、密码、Cookie、UUID、私钥和完整分享链接不得出现在日志、事件和错误中；`deletion_error` 只能是稳定原因标识，不含路径。

## 5. 迁移

- 迁移按整数版本顺序执行；每个迁移必须在事务中运行，失败回滚。
- 迁移前创建数据库备份；迁移失败不得删除原库。
- 不提供自动降级；回滚使用备份或显式的数据迁移。
- 迁移必须保留未知的 JSON 字段或拒绝加载，不得静默丢弃指纹规格。
- 迁移完成后运行 `PRAGMA foreign_key_check` 和完整性检查。
- 应用版本与数据库迁移版本独立；启动时发现数据库版本高于应用支持版本必须拒绝启动。
- 对已发布的数据库，`admin_credentials`、`deletion_error`、`asset_sha256`/`binary_sha256`、`runtimes.isolation` 与 `idx_profile_tags_tag` 这类变更必须先加新表/新列/新索引，再在后续迁移删除旧列；不得在同一迁移里删除并重命名。
- 首个发布无存量库，以下路径针对发布后升级；全部按“加新、回填、切换、后删旧”拆分迁移。
- `kernel_installs`/`proxy_engine_installs`：新增 `asset_sha256`、`binary_sha256`；回填现有文件摘要；新代码只读 `binary_sha256`；最后一个迁移删除 `sha256`。
- `runtimes.proxy_outbound_key`：停止流程必须先写 NULL 再递减计数；迁移先对 `stopped`/`crashed`/`failed` 行置 NULL，再重建 `proxy_outbounds.ref_count`，不一致返回 `PROXY_OUTBOUND_REFCOUNT_MISMATCH`。
- `profiles.window_json`：先回填 `fingerprint_json.window`，再删除旧列；回填时保留用户已设置尺寸，指纹缺少 `window` 的行用旧值补足后走 schema 校验。
- `event_tickets`、`admin_credentials`：新表直接创建，旧 Token 仍有效；`admin_credentials` 为空时进入引导流程，不自动生成固定口令。
- `profiles.deletion_error` 与 `runtimes.isolation`：新列默认 NULL；启动对账把无法确认的历史运行时置 `unknown`，不猜 `enforced`。
- 已发布且标记 `deleted` 的档案在删除 `window_json` 前必须仍有 `fingerprint_json.window`；否则迁移阻断并报告目录 ID，不写默认窗口。

## 6. 保留与清理

| 数据 | 默认保留 |
| --- | --- |
| `event_log` | 7 天或 100 MB，取较小者 |
| `idempotency_keys` | 24 小时 |
| `sessions` | 到期后清理 |
| `event_tickets` | 过期或已使用即清理 |
| `audit_events` | 90 天或按部署配置 |
| 运行时日志 | 每档案 10 MB × 5 文件 |
| `admin_credentials` | 不自动清理；随数据库备份/恢复 |
| 已归档与软删除档案数据（含 `trash/`） | 保留窗口由部署配置决定，永久删除需确认 |

## 7. 参考

- [`ARCHITECTURE.md`](ARCHITECTURE.md)
- [`DESIGN.md`](DESIGN.md)
- [`contracts/profile-export.schema.json`](contracts/profile-export.schema.json)
- [`contracts/fingerprint-spec.schema.json`](contracts/fingerprint-spec.schema.json)
- [`adr/0003-storage-and-secret-protection.md`](adr/0003-storage-and-secret-protection.md)
