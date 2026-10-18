//! The cmux server: engine actor, store, REST API and device auth.
//!
//! Layout (architecture 3):
//! - [`actor`]: the engine actor, deltas, the effect runner trait
//! - [`store`]: data dir lock, atomic saves, journal
//! - [`devices`]: paired devices, sessions, pairing codes
//! - [`api`]: the `/v1` routers (`rest`, `auth`)
//! - [`server`]: listeners and start-up; [`client`]: a tiny local HTTP client
//! - [`term`]: PTY host and terminal hub
//! - [`push`]: Web Push and ntfy notifications
//! - [`config`], [`diag`], [`cli`]
//! - [`setup`] and [`doctor`]: install and check the service

pub mod actor;
pub mod agent;
pub mod api;
pub mod claude;
pub mod cli;
pub mod client;
pub mod config;
pub mod devices;
pub mod diag;
pub mod doctor;
pub mod git;
pub mod push;
pub mod server;
pub mod setup;
pub mod store;
pub mod term;
#[cfg(feature = "tls")]
pub mod tls;

pub use server::{Server, ServerBuilder, ServerError};
