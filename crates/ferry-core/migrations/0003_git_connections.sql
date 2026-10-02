-- Git connections: accounts of a git provider (GitHub, GitLab) connected with
-- an access token. `scopes` is a JSON array of strings.
CREATE TABLE git_connections (
    id               TEXT PRIMARY KEY,
    provider         TEXT NOT NULL,
    base_url         TEXT NOT NULL,
    account          TEXT NOT NULL COLLATE NOCASE,
    account_name     TEXT,
    token            TEXT NOT NULL,
    scopes           TEXT NOT NULL DEFAULT '[]',
    token_expires_at TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    UNIQUE (provider, base_url, account)
);

-- The connection whose token authenticates the clones of a service's
-- repository (NULL = clone without one).
ALTER TABLE services ADD COLUMN git_connection_id TEXT REFERENCES git_connections(id) ON DELETE SET NULL;
