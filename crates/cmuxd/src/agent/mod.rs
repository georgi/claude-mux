//! Agents on the server (architecture 7): the endpoints Claude's hooks and the
//! MCP server talk to, the two helper subcommands, and the effect runner that
//! starts and feeds the `claude` processes.
//!
//! - [`wire`]: Claude's hook payloads and MCP tool arguments into core types
//! - [`endpoint`]: `/v1/agent/hook` and `/v1/agent/tool` (Unix socket only)
//! - [`hook`]: `cmuxd hook <Event>`
//! - [`mcp`]: `cmuxd mcp`, the stdio MCP server
//! - [`runner`]: the [`AgentRuntime`] effect runner (worktrees, spawn, paste, kill)

pub mod endpoint;
pub mod hook;
pub mod mcp;
pub mod runner;
pub mod wire;

pub use runner::AgentRuntime;
