-- gengwiki gateway credit attached to invite codes, in fen. Each redemption copies the amount and
-- gets an llm_keys row once the gateway has issued the key.
ALTER TABLE invite_codes ADD COLUMN gateway_fen INTEGER NOT NULL DEFAULT 0 CHECK (gateway_fen >= 0);
ALTER TABLE invite_redemptions ADD COLUMN gateway_fen INTEGER NOT NULL DEFAULT 0;
ALTER TABLE invite_redemptions ADD COLUMN gateway_key_id TEXT;
ALTER TABLE invite_redemptions ADD COLUMN gateway_error TEXT;
CREATE INDEX idx_invite_redemptions_gateway ON invite_redemptions(redeemed_at) WHERE gateway_fen > 0 AND gateway_key_id IS NULL;
