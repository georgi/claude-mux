//! Connection to a cmuxd server: REST (`http`), the WebSocket thread
//! (`stream`) and the paired-servers file (`servers`).

pub mod cli;
pub mod http;
pub mod servers;
pub mod stream;

#[cfg(test)]
mod it;
