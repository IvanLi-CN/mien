# Mien SQLite 数据库契约

本文固定首个版本的逻辑 schema、约束、索引、迁移和加密边界。它是 `docs/ARCHITECTURE.md` 数据模型的详细化，不是可直接执行的迁移文件；实现时必须用 `sqlx` 迁移或等价机制把 DDL 拆成可重入、不可变的迁移。

## 1. 全局约定

- SQLite 使用 WAL、外键和 `busy_timeout=5000`。
- 主键使用 ULID 文本；时间使用 UTC ISO-8601 文本。
- JSON 字段只保存已经过 schema 校验的非秘密数据。
- 代理凭据、API Token 明文、Cookie、主密钥和私钥不得以明文写入数据库。
- 所有跨表写入必须在一个短事务内完成；不得在持锁时等待进程、网络或共享引擎。
- 连接池为单写者语义；写冲突返回 `STORAGE_BUSY`，由调用方短退避重试。
- 迁移只增不改；任何破坏性变更必须先备份并需要显式确认。

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
  window_json TEXT,
  automation_api INTEGER NOT NULL DEFAULT 0 CHECK (automation_api IN (0,1)),
  runtime_json TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT,
  deleted_at TEXT
);

CREATE INDEX idx_profiles_group ON profiles(group_id);
CREATE INDEX idx_profiles_status_updated ON profiles(status, updated_at DESC);
CREATE INDEX idx_profiles_name ON profiles(name);

CREATE TABLE profile_tags (
  profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
  tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (profile_id, tag_id)
);

CREATE TABLE kernel_installs (
  version TEXT NOT NULL,
  platform TEXT NOT NULL CHECK (platform IN ('macos-arm64','linux-x86_64','windows-x64')),
  path TEXT NOT NULL,
  sha256 TEXT NOT NULL,
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
  sha256 TEXT NOT NULL,
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
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX idx_event_log_type_created ON event_log(type, created_at);

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
  action TEXT NOT NULL,
  target TEXT NOT NULL,
  result TEXT NOT NULL CHECK (result IN ('success','failure','denied')),
  request_id TEXT NOT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL
);

CREATE INDEX idx_audit_created ON audit_events(created_at DESC);
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

- 删除档案默认是软删除：写入 `archived_at`/`deleted_at`，不立即删除浏览器目录；永久删除必须是显式操作。
- 档案生命周期使用 `profiles.status` 的 `active`/`archived`/`deleted`，运行状态只保存在 `runtimes`/`runtime_json`；`start_url`、`window_json` 和 `automation_api` 持久化启动偏好。
- `runtimes.profile_id` 唯一，保证一个档案最多一个受监督运行时；启动/停止通过事务和档案锁串行化。
- `proxy_outbounds.ref_count` 必须与 `runtimes` 中引用该 key 的行数一致；启动时对账并修复，不一致返回 `PROXY_OUTBOUND_REFCOUNT_MISMATCH`。
- 共享引擎状态是单例；`proxy_engine_state.id=1` 固定。
- 一个平台最多一个 active 内核和一个 active 引擎；切换 active 必须在事务中完成。
- `event_log`、`idempotency_keys`、`sessions` 和审计事件按保留策略清理；清理不得删除当前运行时所需的行。
- `event_tickets` 只保存一次性 WebSocket 票据哈希；过期或已使用票据不得复用，按保留策略清理。
- 幂等响应只保存非敏感响应体；如果响应包含一次性 Token 或凭据，不写入 `response_json`。
- `groups.name` 和 `tags.name` 唯一；冲突分别返回 `GROUP_NAME_CONFLICT`/`TAG_NAME_CONFLICT`。审计记录必须带 `request_id`，用于与请求和事件关联。

## 4. 加密与数据边界

- `proxies.config_ciphertext` 使用 AES-256-GCM 字段级加密；关联数据绑定 `proxy_id` 和字段名。
- `credential_fields_json` 只记录已设置的凭据字段名，不记录值。
- `api_tokens.token_hash` 只保存带 pepper 的哈希；明文只在创建响应中出现一次。
- `sessions.token_hash` 只保存随机标识哈希；Cookie 值不落库。
- `fingerprint_json` 和 `runtime_json` 不含秘密。
- 主密钥、代理凭据、Token 明文和完整分享链接不得出现在日志、事件和错误中。

## 5. 迁移

- 迁移按整数版本顺序执行；每个迁移必须在事务中运行，失败回滚。
- 迁移前创建数据库备份；迁移失败不得删除原库。
- 不提供自动降级；回滚使用备份或显式的数据迁移。
- 迁移必须保留未知的 JSON 字段或拒绝加载，不得静默丢弃指纹规格。
- 迁移完成后运行 `PRAGMA foreign_key_check` 和完整性检查。
- 应用版本与数据库迁移版本独立；启动时发现数据库版本高于应用支持版本必须拒绝启动。

## 6. 保留与清理

| 数据 | 默认保留 |
| --- | --- |
| `event_log` | 7 天或 100 MB，取较小者 |
| `idempotency_keys` | 24 小时 |
| `sessions` | 到期后清理 |
| `audit_events` | 90 天或按部署配置 |
| 运行时日志 | 每档案 10 MB × 5 文件 |
| 已归档档案数据 | 保留窗口由部署配置决定，永久删除需确认 |

## 7. 参考

- [`ARCHITECTURE.md`](ARCHITECTURE.md)
- [`DESIGN.md`](DESIGN.md)
- [`contracts/profile-export.schema.json`](contracts/profile-export.schema.json)
- [`contracts/fingerprint-spec.schema.json`](contracts/fingerprint-spec.schema.json)
- [`adr/0003-storage-and-secret-protection.md`](adr/0003-storage-and-secret-protection.md)
