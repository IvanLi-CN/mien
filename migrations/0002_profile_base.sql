-- 0002_profile_base: profile aggregate and its required named resources.
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
  proxy_id TEXT REFERENCES proxies(id) ON DELETE RESTRICT,
  import_source_id TEXT,
  mode TEXT NOT NULL DEFAULT 'headful' CHECK (mode IN ('headful','headless','pseudo_headful')),
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived','deleted')),
  start_url TEXT,
  automation_api INTEGER NOT NULL DEFAULT 0 CHECK (automation_api IN (0,1)),
  runtime_json TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT,
  deleted_at TEXT,
  deletion_error TEXT CHECK (deletion_error IS NULL OR deletion_error IN ('busy','permission_denied','io_error','unknown')),
  CHECK (status <> 'deleted' OR deleted_at IS NOT NULL)
);

CREATE INDEX idx_profiles_group ON profiles(group_id);
CREATE INDEX idx_profiles_status_updated ON profiles(status, updated_at DESC);
CREATE INDEX idx_profiles_name ON profiles(name);
