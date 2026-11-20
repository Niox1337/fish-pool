ALTER TABLE invite_codes ADD COLUMN quota_json TEXT
  CHECK (quota_json IS NULL OR (json_valid(quota_json) AND json_type(quota_json) = 'object'));
