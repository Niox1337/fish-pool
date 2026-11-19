-- Core schema of the fish console.

CREATE TABLE users (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('user', 'admin')),
  created_at INTEGER NOT NULL,
  quota_memory_mb INTEGER NOT NULL DEFAULT 256,
  quota_storage_mb INTEGER NOT NULL DEFAULT 1024,
  quota_bots INTEGER NOT NULL DEFAULT 1,
  quota_plugins INTEGER NOT NULL DEFAULT 8
);

CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  user_agent TEXT
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);

CREATE TABLE email_codes (
  email TEXT NOT NULL,
  purpose TEXT NOT NULL CHECK (purpose IN ('register', 'reset')),
  code_hash TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_email_codes_lookup ON email_codes(email, purpose, created_at);
CREATE INDEX idx_email_codes_expires ON email_codes(expires_at);

CREATE TABLE captchas (
  id TEXT PRIMARY KEY,
  answer_hash TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX idx_captchas_expires ON captchas(expires_at);

CREATE TABLE llm_keys (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  base_url TEXT NOT NULL,
  model TEXT NOT NULL,
  api_key_enc TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_llm_keys_user ON llm_keys(user_id);

CREATE TABLE nodes (
  id TEXT PRIMARY KEY,
  base_url TEXT NOT NULL,
  region TEXT NOT NULL DEFAULT '',
  capacity_memory_mb INTEGER NOT NULL DEFAULT 2048,
  capacity_storage_mb INTEGER NOT NULL DEFAULT 40960,
  created_at INTEGER NOT NULL
);

-- desired_running holds the user's start/stop intent, needs_push marks a desired
-- state that has not reached the node yet.
CREATE TABLE bots (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  avatar TEXT,
  node_id TEXT NOT NULL REFERENCES nodes(id),
  status TEXT NOT NULL DEFAULT 'starting',
  status_detail TEXT NOT NULL DEFAULT '',
  desired_running INTEGER NOT NULL DEFAULT 1,
  needs_push INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  settings_json TEXT NOT NULL
);
CREATE INDEX idx_bots_user ON bots(user_id);
CREATE INDEX idx_bots_node ON bots(node_id);
CREATE INDEX idx_bots_needs_push ON bots(needs_push) WHERE needs_push = 1;

CREATE TABLE platforms (
  id TEXT PRIMARY KEY,
  bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('qq_official', 'weixin_ilink')),
  display_name TEXT NOT NULL DEFAULT '',
  app_id TEXT NOT NULL,
  secret_enc TEXT NOT NULL,
  extra_json TEXT NOT NULL DEFAULT '{}',
  enabled INTEGER NOT NULL DEFAULT 1,
  status TEXT NOT NULL DEFAULT 'connecting',
  status_detail TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_platforms_bot ON platforms(bot_id);

-- install_rev changes whenever the node must reinstall the plugin source.
CREATE TABLE plugins (
  id TEXT PRIMARY KEY,
  bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
  repo_url TEXT NOT NULL,
  name TEXT NOT NULL,
  display_name TEXT NOT NULL DEFAULT '',
  version TEXT NOT NULL DEFAULT '',
  author TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  enabled INTEGER NOT NULL DEFAULT 1,
  config_json TEXT NOT NULL DEFAULT '{}',
  schema_json TEXT,
  status TEXT NOT NULL DEFAULT 'installing',
  status_detail TEXT NOT NULL DEFAULT '',
  install_rev INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_plugins_bot ON plugins(bot_id);
CREATE UNIQUE INDEX idx_plugins_bot_name ON plugins(bot_id, name);

-- Throttling events such as failed logins per email or email code requests per IP.
CREATE TABLE rate_events (
  bucket TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_rate_events_bucket ON rate_events(bucket, created_at);
CREATE INDEX idx_rate_events_created ON rate_events(created_at);

-- QR binding tasks started on a node, owned by one bot.
CREATE TABLE qr_tasks (
  task_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('qq', 'weixin')),
  bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
  node_id TEXT NOT NULL,
  platform_id TEXT,
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (kind, task_id)
);
CREATE INDEX idx_qr_tasks_bot ON qr_tasks(bot_id);

-- Bots deleted in D1 whose node side removal has not succeeded yet.
CREATE TABLE node_tombstones (
  bot_id TEXT PRIMARY KEY,
  node_id TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
