-- SQLite schema for flaps-store v0.1
-- All timestamps stored as ISO-8601 UTC text.

CREATE TABLE IF NOT EXISTS projects (
    key         TEXT NOT NULL PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    external_ref TEXT,
    managed_by  TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_external_ref
    ON projects (external_ref)
    WHERE external_ref IS NOT NULL;

CREATE TABLE IF NOT EXISTS environments (
    project_key  TEXT NOT NULL REFERENCES projects(key) ON DELETE CASCADE,
    key          TEXT NOT NULL,
    name         TEXT NOT NULL,
    external_ref TEXT,
    managed_by   TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    PRIMARY KEY (project_key, key)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_environments_external_ref
    ON environments (external_ref)
    WHERE external_ref IS NOT NULL;

CREATE TABLE IF NOT EXISTS flags (
    project_key   TEXT NOT NULL REFERENCES projects(key) ON DELETE CASCADE,
    key           TEXT NOT NULL,
    name          TEXT NOT NULL,
    description   TEXT,
    flag_type     TEXT NOT NULL,
    value_type    TEXT NOT NULL,
    variants_json TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (project_key, key)
);

CREATE TABLE IF NOT EXISTS segments (
    project_key TEXT NOT NULL REFERENCES projects(key) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    name        TEXT NOT NULL,
    match_json  TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (project_key, key)
);

CREATE TABLE IF NOT EXISTS flag_env_configs (
    project_key     TEXT NOT NULL,
    flag_key        TEXT NOT NULL,
    environment_key TEXT NOT NULL,
    config_json     TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (project_key, flag_key, environment_key),
    FOREIGN KEY (project_key, flag_key)        REFERENCES flags(project_key, key)        ON DELETE CASCADE,
    FOREIGN KEY (project_key, environment_key) REFERENCES environments(project_key, key) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS sdk_keys (
    key_hash        TEXT NOT NULL PRIMARY KEY,
    prefix          TEXT NOT NULL,
    kind            TEXT NOT NULL,
    project_key     TEXT NOT NULL REFERENCES projects(key) ON DELETE CASCADE,
    environment_key TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    FOREIGN KEY (project_key, environment_key) REFERENCES environments(project_key, key) ON DELETE CASCADE
);
