-- 0004_proxy_outbounds: 共享出站引用计数表（docs/DATABASE-SCHEMA.md §2）。
-- runtimes 表已由 0003 建立并含 proxy_outbound_key 列，此处只补外键缺失的引用表，不重建表。
CREATE TABLE proxy_outbounds (
  outbound_key TEXT PRIMARY KEY,
  route_key_json TEXT NOT NULL,
  ref_count INTEGER NOT NULL DEFAULT 0 CHECK (ref_count >= 0),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
