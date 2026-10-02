//! OAuth 2 (authorization code) for GitLab: the account authorizes an
//! OAuth application on GitLab's own page, and the code GitLab sends back is
//! exchanged for an access token and the refresh token that renews it.
//!
//! GitLab has no way to register an application on the fly, so the
//! application (its id and secret) is created once by the user, with the
//! dashboard's `/git/callback` page as its redirect URI.

use ferry_core::GitProvider;
use reqwest::Url;

use crate::provider::{OAuthTokens, Provider, ProviderError};

/// What the application asks for: list the account's projects, and clone them.
pub const GITLAB_SCOPES: &[&str] = &["read_api", "read_repository"];

/// GitLab's page that asks the account to authorize the application.
pub fn authorize_url(
    base_url: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
) -> Result<String, ProviderError> {
    let invalid = || ProviderError::Unavailable(format!("invalid GitLab address {base_url}"));
    let mut url = Url::parse(base_url).map_err(|_| invalid())?;
    url.path_segments_mut().map_err(|_| invalid())?.pop_if_empty().extend(["oauth", "authorize"]);
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("state", state)
        .append_pair("scope", &GITLAB_SCOPES.join(" "));
    Ok(url.into())
}

/// Exchange the code GitLab sent back for tokens. `redirect_uri` must be the
/// one the authorization was asked with.
pub async fn exchange(
    base_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<OAuthTokens, ProviderError> {
    Provider::anonymous(GitProvider::Gitlab, base_url)
        .oauth_token(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ])
        .await
}

/// Renew an access token. The refresh token is spent: the answer carries the
/// next one.
pub async fn refresh(
    base_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<OAuthTokens, ProviderError> {
    Provider::anonymous(GitProvider::Gitlab, base_url)
        .oauth_token(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
        ])
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_urls() {
        let url = authorize_url("https://gitlab.com", "app id", "http://localhost:7878/git/callback", "st4te").unwrap();
        assert_eq!(
            url,
            "https://gitlab.com/oauth/authorize?client_id=app+id&redirect_uri=http%3A%2F%2Flocalhost%3A7878%2Fgit%2Fcallback\
             &response_type=code&state=st4te&scope=read_api+read_repository"
        );
        // A self-managed instance below a path.
        let url = authorize_url("https://example.com/gitlab", "id", "https://ferry.example.com/git/callback", "s");
        assert!(url.unwrap().starts_with("https://example.com/gitlab/oauth/authorize?client_id=id&"));
        assert!(authorize_url("not a url", "id", "r", "s").is_err());
    }
}
