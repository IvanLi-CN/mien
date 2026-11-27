-- 0008_event_log: 一次性 WebSocket 票据与事件回放日志。
--
-- 列与 docs/DATABASE-SCHEMA.md §2 基线 DDL 一致，沿用 IF NOT EXISTS 以便旧库只补齐
-- 缺失的表、不重建也不清空已有行。
-- event_tickets 只保存票据的 SHA-256（ticket_hash），绑定会话或 API Token 且二者
-- 恰有一个非空；升级时必须重新检查绑定对象未撤销。
-- event_log.sequence 是 AUTOINCREMENT 回放游标：high_water 读取 sqlite_sequence，
-- 保证裁剪与重启后序号永不复用；event_id 只用于外部去重。

CREATE TABLE IF NOT EXISTS event_tickets (
  ticket_hash TEXT PRIMARY KEY,
  session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
  token_id TEXT REFERENCES api_tokens(id) ON DELETE CASCADE,
  expires_at TEXT NOT NULL,
  used_at TEXT,
  created_at TEXT NOT NULL,
  CHECK (session_id IS NOT NULL OR token_id IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_event_tickets_expires ON event_tickets(expires_at);

CREATE TABLE IF NOT EXISTS event_log (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_event_log_type_sequence ON event_log(type, sequence);
