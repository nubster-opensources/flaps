-- PostgreSQL audit_log table for flaps-store v0.1
-- Append-only: no UPDATE or DELETE paths exist in the application layer.
-- No FK to audited entities: audit records survive ON DELETE CASCADE.

CREATE TABLE IF NOT EXISTS audit_log (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    before_json JSONB,
    after_json  JSONB,
    occurred_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_log_entity
    ON audit_log (entity_type, entity_id);

CREATE INDEX IF NOT EXISTS idx_audit_log_occurred_at
    ON audit_log (occurred_at);
