//! `ferryd reset-password`: replace the password of the server's account
//! from the machine it runs on, for an administrator who forgot it.
//!
//! Whoever can run this can read the data directory, so nothing else is
//! asked. It works while the server runs: the database is shared.

use std::io::{BufRead, IsTerminal};
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use ferry_core::{Config, Store, auth};

/// The new password: typed twice without echo in a terminal, else the first
/// line of standard input (for scripts).
fn read_new_password(email: &str) -> anyhow::Result<String> {
    if !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).context("reading the new password from standard input")?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    let password = rpassword::prompt_password(format!("New password for {email}: ")).context("reading the password")?;
    let again = rpassword::prompt_password("Again: ").context("reading the password")?;
    if password != again {
        anyhow::bail!("the two passwords differ: nothing was changed");
    }
    Ok(password)
}

/// Set `password` on the account of the database in `data_dir`, and end its
/// sessions. Returns the account's email and how many sessions ended.
pub async fn set_password(data_dir: &Path, password: &str) -> anyhow::Result<(String, u64)> {
    let store = open(data_dir).await?;
    let user = account(&store).await?;
    auth::validate_password(password)?;
    let password = password.to_string();
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&password)).await.context("hashing")??;
    store.set_password(&user.id, &hash).await?;
    let ended = store.delete_sessions(&user.id, None).await?;
    Ok((user.email, ended))
}

async fn open(data_dir: &Path) -> anyhow::Result<Store> {
    let db = Config { data_dir: data_dir.to_path_buf(), ..Config::default() }.db_path();
    if !db.is_file() {
        anyhow::bail!(
            "no Ferry database in {}: give the data directory of the server with --data-dir",
            data_dir.display()
        );
    }
    Store::open(&db).await.context("opening the database")
}

async fn account(store: &Store) -> anyhow::Result<ferry_core::User> {
    store
        .first_user()
        .await?
        .context("this server has no account yet: create it in the dashboard, with the link `ferryd status` prints")
}

pub fn reset_password(data_dir: &Path) -> anyhow::Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().context("starting the runtime")?;
    runtime.block_on(async {
        // Say that there is no account before asking for a password.
        let email = account(&open(data_dir).await?).await?.email;
        let password = read_new_password(&email)?;
        let (email, ended) = set_password(data_dir, &password).await?;
        let sessions = if ended == 1 { "1 session".to_string() } else { format!("{ended} sessions") };
        println!("The password of {email} was changed. Every browser is signed out ({sessions} ended).");
        println!("API tokens keep working: revoke them in the dashboard (Server → Account) if needed.");
        Ok(ExitCode::SUCCESS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_password_is_replaced_and_sessions_end() {
        let dir = std::env::temp_dir().join(format!("ferryd-account-{}", ferry_core::ids::random_secret(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = ferry_core::ids::random_secret(12);

        // Not a data directory.
        let err = set_password(&dir, &fresh).await.unwrap_err();
        assert!(err.to_string().contains("no Ferry database"), "{err}");

        // A server that was never set up.
        let store = open_new(&dir).await;
        let err = set_password(&dir, &fresh).await.unwrap_err();
        assert!(err.to_string().contains("no account yet"), "{err}");

        let old = ferry_core::ids::random_secret(12);
        let user = ferry_core::User::new("ada@example.com", &auth::hash_password(&old).unwrap());
        store.create_first_user(&user).await.unwrap();
        store.create_session(&ferry_core::Session::new(&user.id, "one", None)).await.unwrap();
        store.create_session(&ferry_core::Session::new(&user.id, "two", None)).await.unwrap();

        let err = set_password(&dir, "short").await.unwrap_err();
        assert!(err.to_string().contains("at least 8 characters"), "{err}");
        assert_eq!(set_password(&dir, &fresh).await.unwrap(), ("ada@example.com".to_string(), 2));
        let hash = store.first_user().await.unwrap().unwrap().password_hash;
        assert!(auth::verify_password(&fresh, &hash) && !auth::verify_password(&old, &hash));
        assert!(store.list_sessions(&user.id).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn open_new(dir: &Path) -> Store {
        let db = Config { data_dir: dir.to_path_buf(), ..Config::default() }.db_path();
        Store::open(&db).await.unwrap()
    }
}
