-- Flag and flag-set (environment) metadata (#55).
ALTER TABLE flags ADD COLUMN metadata_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE environments ADD COLUMN metadata_json TEXT NOT NULL DEFAULT '{}';
