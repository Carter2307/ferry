-- The dashboard is signed in to with an account, not with the server's API
-- token (DESIGN.md §20).
--
-- `users` holds the administrator of the server: one row, created by the
-- first-run setup. `password_hash` is an Argon2id PHC string.
CREATE TABLE users (
    id            TEXT PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- A browser that signed in. The cookie holds a random secret; only its
-- SHA-256 is stored, so reading this table doesn't give a session away.
CREATE TABLE sessions (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash   TEXT NOT NULL UNIQUE,
    user_agent   TEXT,
    created_at   TEXT NOT NULL,
    last_used_at TEXT NOT NULL,
    expires_at   TEXT NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);

-- Named API tokens for the CLI and automation, stored as SHA-256 digests
-- like sessions. `hint` is the end of the token, to tell them apart.
CREATE TABLE api_tokens (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    hint         TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    last_used_at TEXT,
    expires_at   TEXT
);
