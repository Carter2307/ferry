-- Ferry schema v1. Timestamps are UTC RFC3339 strings with microseconds
-- (`2026-01-01T12:00:00.000000Z`), so they sort lexicographically.

CREATE TABLE services (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL UNIQUE,
    service_type      TEXT NOT NULL,
    repo_url          TEXT,
    branch            TEXT NOT NULL DEFAULT 'main',
    image             TEXT,
    runtime           TEXT NOT NULL DEFAULT 'auto',
    root_dir          TEXT,
    dockerfile_path   TEXT,
    build_command     TEXT,
    start_command     TEXT,
    publish_dir       TEXT,
    port              INTEGER,
    health_check_path TEXT,
    schedule          TEXT,
    instances         INTEGER NOT NULL DEFAULT 1,
    auto_deploy       INTEGER NOT NULL DEFAULT 1,
    suspended         INTEGER NOT NULL DEFAULT 0,
    disk_mount_path   TEXT,
    custom_domains    TEXT NOT NULL DEFAULT '[]',
    deploy_hook_key   TEXT NOT NULL,
    live_deploy_id    TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

CREATE TABLE deploys (
    id             TEXT PRIMARY KEY,
    service_id     TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    status         TEXT NOT NULL,
    trigger_kind   TEXT NOT NULL,
    source         TEXT NOT NULL,
    commit_sha     TEXT,
    commit_message TEXT,
    image          TEXT,
    port           INTEGER,
    error          TEXT,
    created_at     TEXT NOT NULL,
    started_at     TEXT,
    finished_at    TEXT
);
CREATE INDEX idx_deploys_service ON deploys(service_id, created_at);
CREATE INDEX idx_deploys_status ON deploys(status);

CREATE TABLE job_runs (
    id           TEXT PRIMARY KEY,
    service_id   TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    trigger_kind TEXT NOT NULL,
    command      TEXT,
    image        TEXT,
    status       TEXT NOT NULL,
    exit_code    INTEGER,
    error        TEXT,
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    finished_at  TEXT
);
CREATE INDEX idx_job_runs_service ON job_runs(service_id, created_at);
CREATE INDEX idx_job_runs_status ON job_runs(status);

CREATE TABLE datastores (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    kind       TEXT NOT NULL,
    version    TEXT NOT NULL,
    status     TEXT NOT NULL,
    username   TEXT NOT NULL,
    password   TEXT NOT NULL,
    database   TEXT,
    host_port  INTEGER,
    error      TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE env_groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- owner_id is a service id or an env group id.
CREATE TABLE env_vars (
    owner_id TEXT NOT NULL,
    key      TEXT NOT NULL,
    value    TEXT NOT NULL,
    PRIMARY KEY (owner_id, key)
);

CREATE TABLE service_env_groups (
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    group_id   TEXT NOT NULL REFERENCES env_groups(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    PRIMARY KEY (service_id, group_id)
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
