//! The cmux server: engine actor, store, REST API and device auth.
//!
//! Layout (architecture 3):
//! - [`actor`]: the engine actor, deltas, the effect runner trait
//! - [`store`]: data dir lock, atomic saves, journal
//! - [`devices`]: paired devices, sessions, pairing codes
//! - [`api`]: the `/v1` routers (`rest`, `auth`)
//! - [`server`]: listeners and start-up; [`client`]: a tiny local HTTP client
//! - [`term`]: PTY host and terminal hub
//! - [`config`], [`diag`], [`cli`]

pub mod actor;
pub mod agent;
pub mod api;
pub mod claude;
pub mod cli;
pub mod client;
pub mod config;
pub mod devices;
pub mod diag;
pub mod git;
pub mod server;
pub mod store;
pub mod term;
#[cfg(feature = "tls")]
pub mod tls;

pub use server::{Server, ServerBuilder, ServerError};
