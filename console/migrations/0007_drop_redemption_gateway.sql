-- Invite credit is tracked in gateway_credits.
DROP INDEX idx_invite_redemptions_gateway;
ALTER TABLE invite_redemptions DROP COLUMN gateway_fen;
ALTER TABLE invite_redemptions DROP COLUMN gateway_key_id;
ALTER TABLE invite_redemptions DROP COLUMN gateway_error;
