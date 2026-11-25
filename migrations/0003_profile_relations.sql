-- 0003_profile_relations: tags and runtime rows needed by profile lifecycle checks.
CREATE TABLE profile_tags (
  profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
  tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (profile_id, tag_id)
);

CREATE INDEX idx_profile_tags_tag ON profile_tags(tag_id);

CREATE TABLE runtimes (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL UNIQUE REFERENCES profiles(id) ON DELETE CASCADE,
  state TEXT NOT NULL CHECK (state IN ('starting','running','stopping','stopped','crashed','failed')),
  mode TEXT NOT NULL CHECK (mode IN ('headful','headless','pseudo_headful')),
  isolation TEXT NOT NULL DEFAULT 'unknown' CHECK (isolation IN ('enforced','unsandboxed','unknown')),
  last_known_isolation TEXT NOT NULL DEFAULT 'unknown' CHECK (last_known_isolation IN ('enforced','unsandboxed','unknown')),
  pid INTEGER,
  pgid INTEGER,
  proxy_port INTEGER,
  proxy_inbound_tag TEXT,
  proxy_outbound_key TEXT,
  proxy_rule_tag TEXT,
  cdp_port INTEGER,
  started_at TEXT,
  exit_code INTEGER,
  last_error TEXT,
  updated_at TEXT NOT NULL
);

CREATE INDEX idx_runtimes_state ON runtimes(state);
