-- 0006_api_tokens: API Token 持久化与 tokensCreate 幂等键。
--
-- api_tokens 与 idempotency_keys 的列与 docs/DATABASE-SCHEMA.md §2 基线 DDL 一致。
-- token_hash 只保存 HMAC-SHA-256(pepper, token)，pepper 由主密钥经
-- HKDF-SHA256(info="mien/api-token-pepper/v1") 派生且不单独落盘（docs/SECURITY.md §5.2）；
-- token_prefix 只保存 4–16 字符展示前缀，不能用于认证。
-- tokensCreate 的一次性明文永不写入 idempotency_keys.response_json：重复键一律
-- 返回 IDEMPOTENCY_CONFLICT，不重放创建响应。

CREATE TABLE api_tokens (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  token_prefix TEXT CHECK (token_prefix IS NULL OR length(token_prefix) BETWEEN 4 AND 16),
  token_hash TEXT NOT NULL UNIQUE,
  scopes_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_used_at TEXT,
  revoked_at TEXT
);

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
