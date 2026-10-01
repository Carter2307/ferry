//! Resource identifiers.
//!
//! Ferry ids look like Render's: a short type prefix plus 20 random lowercase
//! hex characters, e.g. `srv-3f9a0c1b2d4e5f60718a`. They are DNS/Docker safe.

/// Prefix for services.
pub const SERVICE: &str = "srv";
/// Prefix for deploys.
pub const DEPLOY: &str = "dep";
/// Prefix for job runs (cron runs and one-off jobs).
pub const JOB: &str = "job";
/// Prefix for datastores (Postgres / Redis).
pub const DATASTORE: &str = "dbs";
/// Prefix for environment groups.
pub const ENV_GROUP: &str = "evg";
/// Prefix for git connections (connected GitHub / GitLab accounts).
pub const GIT_CONNECTION: &str = "git";
/// Prefix for domains (services are served at `<service>.<domain>`).
pub const DOMAIN: &str = "dom";

/// Prefix for accounts (the administrator of the server).
pub const USER: &str = "usr";
/// Prefix for sessions (a browser signed in to the dashboard).
pub const SESSION: &str = "ses";
/// Prefix for API tokens.
pub const API_TOKEN: &str = "tok";

/// Generate a new id with the given prefix.
pub fn new_id(prefix: &str) -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}", &raw[..20])
}

/// Generate a random secret of `len` lowercase hex characters (max 64).
pub fn random_secret(len: usize) -> String {
    let mut out = String::with_capacity(len);
    while out.len() < len {
        out.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    out.truncate(len);
    out
}

/// The last 8 characters of an id — handy for container names.
pub fn short(id: &str) -> &str {
    let n = id.len();
    if n <= 8 { id } else { &id[n - 8..] }
}

/// True if `s` looks like an id with the given prefix.
pub fn has_prefix(s: &str, prefix: &str) -> bool {
    s.len() == prefix.len() + 21 && s.starts_with(prefix) && s.as_bytes()[prefix.len()] == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_expected_shape() {
        let id = new_id(SERVICE);
        assert_eq!(id.len(), 24);
        assert!(has_prefix(&id, SERVICE));
        assert!(!has_prefix("web", SERVICE));
        assert_eq!(short(&id).len(), 8);
        assert_eq!(random_secret(40).len(), 40);
    }
}
