-- SDK key soft revocation.
ALTER TABLE sdk_keys ADD COLUMN IF NOT EXISTS revoked_at TEXT NULL;
