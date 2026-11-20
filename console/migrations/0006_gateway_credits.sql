-- One gengwiki gateway key per user. It holds every credit the user is given.
CREATE TABLE gateway_accounts (
  user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  remote_key_id TEXT NOT NULL,
  llm_key_id TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

-- Credits given to a user, each with its own expiry. The gateway bills the one that expires first.
-- remote_id is set once the gateway holds the credit, error keeps the last failed attempt.
CREATE TABLE gateway_credits (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  amount_fen INTEGER NOT NULL CHECK (amount_fen > 0),
  expires_at INTEGER,
  source TEXT NOT NULL CHECK (source IN ('invite', 'admin')),
  ref TEXT NOT NULL UNIQUE,
  note TEXT NOT NULL DEFAULT '',
  created_by TEXT,
  created_at INTEGER NOT NULL,
  remote_id TEXT,
  error TEXT
);
CREATE INDEX idx_gateway_credits_user ON gateway_credits(user_id, created_at);
CREATE INDEX idx_gateway_credits_pending ON gateway_credits(created_at) WHERE remote_id IS NULL;
