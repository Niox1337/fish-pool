-- Admin support and subscription periods granted by invite codes.
ALTER TABLE users ADD COLUMN last_login_at INTEGER;
ALTER TABLE users ADD COLUMN service_until INTEGER;
ALTER TABLE nodes ADD COLUMN last_seen_at INTEGER;
CREATE INDEX idx_users_created ON users(created_at);
CREATE INDEX idx_users_service ON users(service_until);
CREATE INDEX idx_bots_status ON bots(status);
CREATE INDEX idx_plugins_status ON plugins(status);

CREATE TABLE invite_codes (
  code TEXT PRIMARY KEY,
  days INTEGER NOT NULL CHECK (days BETWEEN 1 AND 3650),
  max_uses INTEGER NOT NULL DEFAULT 1 CHECK (max_uses >= 1),
  used_count INTEGER NOT NULL DEFAULT 0,
  note TEXT NOT NULL DEFAULT '',
  created_by TEXT,
  created_at INTEGER NOT NULL,
  expires_at INTEGER,
  revoked_at INTEGER
);
CREATE INDEX idx_invite_codes_created ON invite_codes(created_at);

CREATE TABLE invite_redemptions (
  code TEXT NOT NULL REFERENCES invite_codes(code),
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  days INTEGER NOT NULL,
  redeemed_at INTEGER NOT NULL
);
CREATE INDEX idx_invite_redemptions_code ON invite_redemptions(code);
CREATE INDEX idx_invite_redemptions_user ON invite_redemptions(user_id);
CREATE INDEX idx_invite_redemptions_time ON invite_redemptions(redeemed_at);
