//! `ferryd` — the Ferry server. Wires together the store, Docker, builder,
//! proxy, optional TLS manager, engine and API.
//!
//! Layout: [`cli`] (clap syntax), [`config`] (the options as a `Config`),
//! [`server`] (`ferryd run`: the wiring), [`ownership`] (the data directory's
//! lock, the owner of the Docker name prefix), [`banner`] (what the server
//! says when it starts), [`oom`] (the server's OOM score), [`fsutil`] (the
//! private data directory and its secret files), [`daemon`] (`start`, `stop`,
//! `status`, `logs`) and [`account`] (`reset-password`).

mod account;
mod banner;
mod cli;
mod config;
mod daemon;
mod fsutil;
mod oom;
mod ownership;
mod server;

use std::path::Path;
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command, DEFAULT_DATA_DIR, ServerArgs};

fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    if cli.dump_openapi {
        println!("{}", ferry_api::openapi::document_json());
        return Ok(ExitCode::SUCCESS);
    }
    if cli.dump_markdown_help {
        print!("{}", clap_markdown::help_markdown::<Cli>());
        return Ok(ExitCode::SUCCESS);
    }
    // The commands printed as hints name a data directory that the next
    // `ferryd` wouldn't find by itself.
    let hint = |dir: &Path| {
        let found = dir == Path::new(DEFAULT_DATA_DIR) || std::env::var_os("FERRY_DATA_DIR").is_some();
        daemon::data_dir_hint(dir, found)
    };
    let start = |args: ServerArgs| {
        fsutil::secure_dir(&args.data_dir)?;
        daemon::start(&args.data_dir, &cli::server_args(std::env::args_os()), &hint(&args.data_dir))
    };
    match cli.command {
        Some(Command::Start(args)) => start(args),
        Some(Command::Run { server: args, detached }) => server::run(args, detached),
        None if cli::interactive() => start(cli.server),
        None => server::run(cli.server, false),
        Some(Command::Stop(dir)) => daemon::stop(&dir.data_dir),
        Some(Command::Status(dir)) => daemon::status(&dir.data_dir, &hint(&dir.data_dir)),
        Some(Command::Logs { data_dir, follow, lines }) => daemon::logs(&data_dir.data_dir, lines, follow),
        Some(Command::ResetPassword(dir)) => account::reset_password(&dir.data_dir),
    }
}
