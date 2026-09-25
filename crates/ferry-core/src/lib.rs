//! # ferry-core
//!
//! The shared vocabulary of Ferry: domain models, API DTOs, configuration,
//! persistence (SQLite), environment-variable resolution, log plumbing and the
//! [`Engine`] trait that decouples the HTTP API from the deployment engine.
//!
//! Every other crate depends on this one; this crate depends on no other Ferry crate.

pub mod config;
pub mod dto;
pub mod engine;
pub mod env;
pub mod error;
pub mod git;
pub mod ids;
pub mod logs;
pub mod models;
pub mod naming;
pub mod schedule;
pub mod store;
pub mod tls;
pub mod validate;

pub use config::Config;
pub use engine::{DeployRequest, Engine, LogOptions};
pub use error::{Error, Result};
pub use logs::{LogLine, LogSink, LogStream, LogStreamKind};
pub use models::*;
pub use naming::Naming;
pub use store::Store;
pub use tokio_util::sync::CancellationToken;

/// Ferry version (from Cargo).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
