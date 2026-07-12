CREATE TABLE IF NOT EXISTS ai_profiles (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  provider TEXT NOT NULL,
  auth_mode TEXT NOT NULL DEFAULT 'api_key',
  base_url TEXT,
  model TEXT NOT NULL,
  temperature REAL NOT NULL DEFAULT 0.3,
  keep_alive TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  priority INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai_credentials (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL REFERENCES ai_profiles(id) ON DELETE CASCADE,
  label TEXT NOT NULL,
  secret_ref TEXT NOT NULL UNIQUE,
  masked_suffix TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  priority INTEGER NOT NULL DEFAULT 0,
  state TEXT NOT NULL DEFAULT 'active',
  cooldown_until INTEGER,
  last_error_kind TEXT,
  last_used_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ai_profiles_active
  ON ai_profiles(enabled, priority);
CREATE INDEX IF NOT EXISTS idx_ai_credentials_profile_priority
  ON ai_credentials(profile_id, enabled, priority);
