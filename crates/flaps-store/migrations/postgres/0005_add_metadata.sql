-- Flag and flag-set (environment) metadata (#55).
ALTER TABLE flags ADD COLUMN IF NOT EXISTS metadata_json JSONB NOT NULL DEFAULT '{}';
ALTER TABLE environments ADD COLUMN IF NOT EXISTS metadata_json JSONB NOT NULL DEFAULT '{}';
