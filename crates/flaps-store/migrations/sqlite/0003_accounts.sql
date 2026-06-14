-- Accounts and sessions for local authentication.
-- All timestamps stored as ISO-8601 UTC text.

CREATE TABLE IF NOT EXISTS accounts (
    id          TEXT NOT NULL PRIMARY KEY,
    username    TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    is_active   INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    token_hash  TEXT NOT NULL PRIMARY KEY,
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    expires_at  TEXT NOT NULL,
    revoked_at  TEXT
);
