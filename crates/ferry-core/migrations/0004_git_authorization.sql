-- Git connections are authorized in the browser and belong to the server, not
-- to a service: a repository is cloned with the connection that serves its URL.
--
-- `auth` says where the tokens of a connection come from:
--   github_app  a GitHub App registered for this server (`app_id`, `app_slug`,
--               `app_url`, `client_id`, `client_secret`, `private_key`,
--               `webhook_secret`) and installed on the account
--               (`installation_id`): tokens are minted on demand, never stored
--   oauth       an OAuth application (`client_id`, `client_secret`) the account
--               authorized: `token` is the current access token, renewed with
--               `refresh_token` before `token_expires_at`
--   token       a personal access token (`token`): every connection made so far
-- A `github_app` connection without an installation, or an `oauth` one without
-- a token, is pending: its authorization was started but not finished (`token`
-- is '' then, and `account` too until an OAuth application was authorized).
ALTER TABLE git_connections ADD COLUMN auth TEXT NOT NULL DEFAULT 'token';
ALTER TABLE git_connections ADD COLUMN refresh_token TEXT;
ALTER TABLE git_connections ADD COLUMN client_id TEXT;
ALTER TABLE git_connections ADD COLUMN client_secret TEXT;
ALTER TABLE git_connections ADD COLUMN app_id INTEGER;
ALTER TABLE git_connections ADD COLUMN app_slug TEXT;
ALTER TABLE git_connections ADD COLUMN app_url TEXT;
ALTER TABLE git_connections ADD COLUMN private_key TEXT;
ALTER TABLE git_connections ADD COLUMN webhook_secret TEXT;
ALTER TABLE git_connections ADD COLUMN installation_id INTEGER;
ALTER TABLE git_connections ADD COLUMN installation_url TEXT;
ALTER TABLE git_connections ADD COLUMN repository_selection TEXT;

-- A service no longer names a connection.
ALTER TABLE services DROP COLUMN git_connection_id;
