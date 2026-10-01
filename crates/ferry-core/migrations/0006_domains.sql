-- Domains services are served under: `<service>.<name>` (DESIGN.md §21).
--
-- `source` is `config` for the server's `--base-domain` (one row, kept in
-- step with the flag at every start) and `connected` for a domain added
-- through the API. `status` says whether the names under it reach this
-- server; `checks` is the JSON outcome of the last verification and
-- `failures` the number of verifications that failed in a row.
CREATE TABLE domains (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    source      TEXT NOT NULL,
    status      TEXT NOT NULL,
    is_default  INTEGER NOT NULL DEFAULT 0,
    checks      TEXT NOT NULL DEFAULT '[]',
    failures    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    verified_at TEXT,
    checked_at  TEXT
);

-- One default domain at most: the domain of the URL a service is shown with.
CREATE UNIQUE INDEX domains_default ON domains(is_default) WHERE is_default = 1;
