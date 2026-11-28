-- 0001_init: 仅建立迁移账本。业务表由 T9-A 按 docs/DATABASE-SCHEMA.md 追加到后续迁移。
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  applied_at TEXT NOT NULL
);
