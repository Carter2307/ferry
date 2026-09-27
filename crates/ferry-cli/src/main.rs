//! `ferry` — command-line client for a Ferry server (DESIGN.md §12).
//!
//! Layout: [`cli`] (clap syntax), [`config`] (saved login + env/flag
//! overrides), [`client`] (HTTP + SSE), [`sse`] (event-stream parser),
//! [`archive`] (`ferry up` tarballs), [`repo`] (local repository paths),
//! [`envref`] (env var reference checks), [`output`] (tables, colors, times)
//! and [`commands`] (one module per command family).
//!
//! Exit codes: 0 success, 1 failure (API error, failed deploy/job, refused
//! confirmation), 2 usage error (clap), 130 interrupted with Ctrl-C.

mod archive;
mod cli;
mod client;
mod commands;
mod config;
mod envref;
mod output;
mod repo;
mod sse;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::client::ApiError;
use crate::commands::Exit;
use crate::config::{Overrides, Settings};
use crate::output::{Color, errln};

fn main() -> ExitCode {
    let cli = Cli::parse();
    if matches!(cli.command, Command::MarkdownHelp) {
        print!("{}", clap_markdown::help_markdown::<Cli>());
        return ExitCode::SUCCESS;
    }
    let settings = match load_settings(&cli) {
        Ok(s) => s,
        Err(e) => return report(&e),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            errln!("{} starting the async runtime: {e}", output::err(Color::Red, "error:"));
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        let interrupted = async {
            // If the handler can't be installed, never report an interrupt.
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            r = commands::run(cli, settings) => Some(r),
            () = interrupted => None,
        }
    });
    // Don't wait for abandoned blocking work (e.g. a confirmation prompt
    // still reading stdin after Ctrl-C).
    runtime.shutdown_timeout(Duration::from_secs(1));
    match result {
        Some(Ok(())) => ExitCode::SUCCESS,
        Some(Err(e)) => report(&e),
        None => {
            errln!();
            if let Some(hint) = output::take_interrupt_hint() {
                errln!("{hint}");
            }
            ExitCode::from(130)
        }
    }
}

/// Resolve the connection settings (flags > env > config file > default).
/// An unreadable config file is an error, except for `login`, which
/// overwrites it.
fn load_settings(cli: &Cli) -> Result<Settings> {
    let file = match config::config_path() {
        Some(path) => match config::load(&path) {
            Ok(f) => f,
            Err(e) if matches!(cli.command, Command::Login) => {
                errln!("warning: ignoring {e:#}");
                None
            }
            Err(e) => {
                return Err(anyhow::anyhow!("{e:#} (fix or delete the file, or run 'ferry login' to rewrite it)"));
            }
        },
        None => None,
    };
    let flags = Overrides { server: cli.global.server.clone(), token: cli.global.token.clone() };
    config::resolve(&flags, |k| std::env::var(k).ok(), file.as_ref())
}

/// Print an error (unless it is a silent exit) and pick the exit code.
fn report(err: &anyhow::Error) -> ExitCode {
    if let Some(Exit(code)) = err.downcast_ref::<Exit>() {
        return ExitCode::from(*code);
    }
    // Output piped into a closed reader (`ferry logs web | head`): not an error.
    if err
        .chain()
        .any(|e| e.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe))
    {
        return ExitCode::SUCCESS;
    }
    errln!("{} {err:#}", output::err(Color::Red, "error:"));
    if let Some(api) = err.chain().find_map(|e| e.downcast_ref::<ApiError>())
        && api.status == 401
    {
        errln!("hint: check the server URL and token with 'ferry login --server <URL> --token <TOKEN>'");
    }
    ExitCode::FAILURE
}
