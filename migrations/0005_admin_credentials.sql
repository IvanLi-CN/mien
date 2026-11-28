-- 0005_admin_credentials: 管理员凭据、会话与认证审计。
--
-- admin_credentials 与 sessions 的列与 docs/DATABASE-SCHEMA.md §2 基线 DDL 一致。
-- 每会话 CSRF Token 由 Cookie Token 经 HKDF 派生（见 auth::csrf），因此不额外落库。
-- audit_events 同样采用基线 DDL：T22 的改密流程必须写审计事件，而此前没有任何
-- 迁移创建该表；此处使用 IF NOT EXISTS，后续审计任务可复用同一表定义。

CREATE TABLE IF NOT EXISTS admin_credentials (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  password_hash TEXT NOT NULL,
  algo TEXT NOT NULL CHECK (algo = 'argon2id'),
  params_json TEXT NOT NULL,
  must_change INTEGER NOT NULL DEFAULT 1 CHECK (must_change IN (0,1)),
  bootstrap_pending INTEGER NOT NULL DEFAULT 0 CHECK (bootstrap_pending IN (0,1)),
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  last_used_at TEXT,
  revoked_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);

CREATE TABLE IF NOT EXISTS audit_events (
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

CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_events(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_event_type_created ON audit_events(event_type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_profile_created ON audit_events(profile_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_action_created ON audit_events(action, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_request ON audit_events(request_id);
