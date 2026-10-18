//! Wire types shared by the server, the TUI, the CLI and (through generated
//! TypeScript) the web app. Types only: no I/O, no server or client logic.
//!
//! - [`view`]: client-facing snapshots and the delta the stream carries.
//! - [`api`]: REST bodies, header names, scopes, routes.
//! - [`stream`]: WebSocket control messages and the binary terminal frames.
//!
//! Ids and the plain state enums come from `cmux-core` and are re-exported
//! from [`view`], so there is one spelling of each on the wire.

pub mod api;
pub mod stream;
pub mod view;

/// The flags every `claude` that cmux starts gets first: Remote Control and
/// auto mode. `--remote-control` takes an optional name, so a flag must follow
/// it, never a prompt. Flags that come later (the caller's own) win.
pub const CLAUDE_DEFAULT_ARGS: &[&str] = &["--remote-control", "--permission-mode", "auto"];

/// The API version prefix every route lives under.
pub const API_PREFIX: &str = "/v1";
