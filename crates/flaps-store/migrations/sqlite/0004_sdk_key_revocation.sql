-- SDK key soft revocation.
ALTER TABLE sdk_keys ADD COLUMN revoked_at TEXT NULL;
