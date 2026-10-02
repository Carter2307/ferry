//! Accounts (DESIGN.md §20): the administrator's password, and the secrets
//! of sessions and API tokens.
//!
//! A password is stored as an Argon2id hash. Session and API tokens are
//! random (about 150 bits), so the store only keeps their SHA-256: a fast
//! digest is enough when there is nothing to guess.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use sha2::{Digest, Sha256};

use crate::{Error, Result, ids};

/// Shortest password an account accepts.
pub const MIN_PASSWORD_LEN: usize = 8;
/// Longest one: hashing is deliberately slow, and its cost grows with the input.
pub const MAX_PASSWORD_LEN: usize = 256;
/// A session ends this long after it was last used.
pub const SESSION_DAYS: i64 = 30;
/// What API tokens (and the server token) start with.
pub const API_TOKEN_PREFIX: &str = "fy_";
/// What session secrets start with.
pub const SESSION_PREFIX: &str = "fys_";

/// The email of an account, as stored and compared: trimmed, lowercase.
pub fn normalize_email(email: &str) -> Result<String> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= 254
        && !email.contains(char::is_whitespace)
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@') && !domain.starts_with('.')
        });
    if !valid {
        return Err(Error::invalid("enter a valid email address"));
    }
    Ok(email)
}

/// Check a new password (its length, counted in characters).
pub fn validate_password(password: &str) -> Result<()> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(Error::invalid(format!("the password must have at least {MIN_PASSWORD_LEN} characters")));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(Error::invalid(format!("the password must have at most {MAX_PASSWORD_LEN} characters")));
    }
    Ok(())
}

/// Hash a password for storage (Argon2id, a random salt, PHC string format).
/// Slow on purpose: call it from a blocking thread.
pub fn hash_password(password: &str) -> Result<String> {
    // A salt only has to be unique: 16 bytes of a random UUID are.
    let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes())
        .map_err(|e| Error::internal(format!("hashing the password: {e}")))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| Error::internal(format!("hashing the password: {e}")))
}

/// Does `password` match a stored hash? As slow as [`hash_password`].
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
}

/// A new API token: `fy_` and 40 random hex characters.
pub fn new_api_token() -> String {
    format!("{API_TOKEN_PREFIX}{}", ids::random_secret(40))
}

/// A new session secret (the value of the session cookie).
pub fn new_session_secret() -> String {
    format!("{SESSION_PREFIX}{}", ids::random_secret(48))
}

/// What the store keeps of a session or API token: its SHA-256, in hex.
pub fn digest(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// The end of a token, shown next to its name to tell tokens apart.
pub fn hint(token: &str) -> String {
    let start = token.char_indices().rev().nth(3).map_or(0, |(i, _)| i);
    token[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_normalized() {
        assert_eq!(normalize_email("  Ada@Example.COM ").unwrap(), "ada@example.com");
        for bad in ["", "ada", "@example.com", "ada@", "a da@example.com", "ada@@example.com", "ada@.com"] {
            assert!(normalize_email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn passwords_have_a_length() {
        assert!(validate_password("1234567").is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("pâté-crêpe").is_ok(), "characters, not bytes");
        assert!(validate_password(&"x".repeat(MAX_PASSWORD_LEN)).is_ok());
        assert!(validate_password(&"x".repeat(MAX_PASSWORD_LEN + 1)).is_err());
    }

    #[test]
    fn password_hashes_verify() {
        let secret = ids::random_secret(16);
        let hash = hash_password(&secret).unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(verify_password(&secret, &hash));
        assert!(!verify_password(&format!("{secret}x"), &hash));
        assert_ne!(hash, hash_password(&secret).unwrap(), "a salt per hash");
        assert!(!verify_password(&secret, "not a hash"));
        assert!(!verify_password(&secret, ""));
    }

    #[test]
    fn tokens_and_digests() {
        let token = new_api_token();
        assert_eq!(token.len(), 43);
        assert!(token.starts_with(API_TOKEN_PREFIX));
        assert_ne!(token, new_api_token());
        assert!(new_session_secret().starts_with(SESSION_PREFIX));
        assert_eq!(digest(&token).len(), 64);
        assert_eq!(digest(&token), digest(&token));
        assert_ne!(digest(&token), digest(&new_api_token()));
        assert_eq!(hint(&token), &token[39..]);
        assert_eq!(hint("abc"), "abc");
    }
}
