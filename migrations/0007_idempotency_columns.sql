-- 0007_idempotency_columns: replay columns for non-secret profile/runtime writes.
--
-- `0006_api_tokens.sql` owns the `idempotency_keys` table (key, scope,
-- request_hash, response_json, created_at, expires_at). REQ-API-010 also needs
-- to distinguish "same key, different request" for start/stop/create/duplicate,
-- which requires the request method, path and the replayed status code. Those
-- are additive columns, so this migration only alters the existing table.

ALTER TABLE idempotency_keys ADD COLUMN method TEXT;
ALTER TABLE idempotency_keys ADD COLUMN path TEXT;
ALTER TABLE idempotency_keys ADD COLUMN status INTEGER;
