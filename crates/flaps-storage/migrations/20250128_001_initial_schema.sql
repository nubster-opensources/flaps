-- Flaps Initial Schema
-- Compatible with PostgreSQL and SQLite
--
-- Note: tenants, groups, and projects are managed by the Workspace API.
-- This schema only stores Flaps-specific data (flags, segments, environments).
-- References to project_id and tenant_id are UUIDs from Workspace without FK constraints.

-- =============================================================================
-- Environments (dev, staging, prod, etc.)
-- =============================================================================
CREATE TABLE IF NOT EXISTS environments (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL, -- References Workspace project
    key TEXT NOT NULL,
    name TEXT NOT NULL,
    color TEXT,
    is_production BOOLEAN NOT NULL DEFAULT FALSE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(project_id, key)
);

CREATE INDEX IF NOT EXISTS idx_environments_project ON environments(project_id);

-- =============================================================================
-- Flags (Feature flags)
-- =============================================================================
CREATE TABLE IF NOT EXISTS flags (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL, -- References Workspace project
    key TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    flag_type TEXT NOT NULL DEFAULT 'boolean', -- 'boolean' or 'string'
    variants TEXT, -- JSON array for string variants
    tags TEXT, -- JSON array of tags
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_by TEXT NOT NULL,
    UNIQUE(project_id, key)
);

CREATE INDEX IF NOT EXISTS idx_flags_project ON flags(project_id);
CREATE INDEX IF NOT EXISTS idx_flags_key ON flags(key);

-- =============================================================================
-- Flag Environments (Per-environment configuration)
-- =============================================================================
CREATE TABLE IF NOT EXISTS flag_environments (
    id TEXT PRIMARY KEY,
    flag_id TEXT NOT NULL REFERENCES flags(id) ON DELETE CASCADE,
    environment_id TEXT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    default_value TEXT NOT NULL, -- JSON value
    rollout_percentage INTEGER, -- 0-100, NULL means 100%
    requires_approval BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_by TEXT,
    UNIQUE(flag_id, environment_id)
);

CREATE INDEX IF NOT EXISTS idx_flag_env_flag ON flag_environments(flag_id);
CREATE INDEX IF NOT EXISTS idx_flag_env_environment ON flag_environments(environment_id);

-- =============================================================================
-- Targeting Rules (Rules within flag environments)
-- =============================================================================
CREATE TABLE IF NOT EXISTS targeting_rules (
    id TEXT PRIMARY KEY,
    flag_environment_id TEXT NOT NULL REFERENCES flag_environments(id) ON DELETE CASCADE,
    priority INTEGER NOT NULL DEFAULT 0,
    value TEXT NOT NULL, -- JSON value to return when matched
    rollout_percentage INTEGER, -- 0-100 for rule-specific rollout
    description TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_rules_flag_env ON targeting_rules(flag_environment_id);

-- =============================================================================
-- Rule Conditions (Conditions for targeting rules)
-- =============================================================================
CREATE TABLE IF NOT EXISTS rule_conditions (
    id TEXT PRIMARY KEY,
    rule_id TEXT NOT NULL REFERENCES targeting_rules(id) ON DELETE CASCADE,
    attribute TEXT NOT NULL,
    operator TEXT NOT NULL,
    value TEXT NOT NULL, -- JSON value
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_conditions_rule ON rule_conditions(rule_id);

-- =============================================================================
-- Segments (Reusable user segments)
-- =============================================================================
CREATE TABLE IF NOT EXISTS segments (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL, -- References Workspace project
    key TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    included_users TEXT, -- JSON array of user IDs
    excluded_users TEXT, -- JSON array of user IDs
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_by TEXT NOT NULL,
    UNIQUE(project_id, key)
);

CREATE INDEX IF NOT EXISTS idx_segments_project ON segments(project_id);

-- =============================================================================
-- Segment Rules (Rules that define segment membership)
-- =============================================================================
CREATE TABLE IF NOT EXISTS segment_rules (
    id TEXT PRIMARY KEY,
    segment_id TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_segment_rules_segment ON segment_rules(segment_id);

-- =============================================================================
-- Segment Conditions (Conditions for segment rules)
-- =============================================================================
CREATE TABLE IF NOT EXISTS segment_conditions (
    id TEXT PRIMARY KEY,
    segment_rule_id TEXT NOT NULL REFERENCES segment_rules(id) ON DELETE CASCADE,
    attribute TEXT NOT NULL,
    operator TEXT NOT NULL,
    value TEXT NOT NULL, -- JSON value
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_segment_conditions_rule ON segment_conditions(segment_rule_id);

-- =============================================================================
-- Audit Log (Track all changes)
-- =============================================================================
CREATE TABLE IF NOT EXISTS audit_log (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL, -- References Workspace tenant
    project_id TEXT, -- References Workspace project (optional, for tenant-level events)
    entity_type TEXT NOT NULL, -- 'flag', 'segment', 'environment', etc.
    entity_id TEXT NOT NULL,
    action TEXT NOT NULL, -- 'create', 'update', 'delete', 'toggle'
    actor_id TEXT NOT NULL,
    actor_type TEXT NOT NULL DEFAULT 'user', -- 'user', 'api_key', 'system'
    changes TEXT, -- JSON diff of changes
    metadata TEXT, -- JSON additional context
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_audit_tenant ON audit_log(tenant_id);
CREATE INDEX IF NOT EXISTS idx_audit_project ON audit_log(project_id);
CREATE INDEX IF NOT EXISTS idx_audit_entity ON audit_log(entity_type, entity_id);
CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_log(actor_id);
CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_log(created_at);

-- =============================================================================
-- API Keys (For SDK authentication)
-- =============================================================================
CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL, -- References Workspace project
    environment_id TEXT REFERENCES environments(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE, -- Hashed API key
    key_prefix TEXT NOT NULL, -- First 8 chars for identification
    permissions TEXT NOT NULL DEFAULT 'read', -- 'read', 'write', 'admin'
    last_used_at TIMESTAMP,
    expires_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_by TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_keys_project ON api_keys(project_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
