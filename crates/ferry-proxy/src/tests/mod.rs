//! Socket-level tests of the proxy: real listeners and upstream servers on
//! 127.0.0.1 with ephemeral ports (no Docker needed).

mod proxy;
mod shutdown;
mod streaming;
mod support;
mod tls;
mod websocket;
