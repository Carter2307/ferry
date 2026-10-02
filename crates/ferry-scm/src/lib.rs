//! # ferry-scm
//!
//! Git providers (GitHub, GitLab) as Ferry sees them: accounts whose
//! repositories this server is authorized to read. Connections belong to
//! the server, not to a service: a repository is cloned with the connection
//! that serves its URL ([`ferry_core::git_connection_for`]).
//!
//! * [`authorize`] — connecting an account in the browser: on GitHub the
//!   server registers a GitHub App for itself and the account installs it
//!   ([`github_app`]); on GitLab the account authorizes an OAuth application
//!   ([`oauth`]). Connecting with a personal access token is there too.
//! * [`access`] — a token to read repositories with right now, renewed or
//!   minted as needed, and the repositories a connection can read.
//! * [`provider`] — the providers' HTTP APIs.
//!
//! Used by the API (the authorization steps, repository listings) and by the
//! engine (the credentials of a clone). No secret leaves through an error
//! message or a log line.

pub mod access;
pub mod authorize;
pub mod github_app;
pub mod oauth;
pub mod provider;

pub use access::{RepoAccess, access_hint, repo_access, repositories};
pub use authorize::{FlowError, Step, callback, connect_with_token, disconnect, instance_url, start};
pub use provider::ProviderError;
