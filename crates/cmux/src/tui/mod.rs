//! Plan features of the TUI (UI design 3.2 to 3.6): pure drawing, navigation,
//! hit-testing and forms. Everything here is a function of a
//! [`cmux_proto::view::StateView`] plus a small [`state::ViewState`]; nothing
//! talks to the network or reads the terminal, so the client's event loop
//! owns those and calls in.
//!
//! - [`state`]: the view state (selection, collapsed nodes, seen flags, focus).
//! - [`rows`]: the flat list of visible tree rows, glyphs and needs-you labels.
//! - [`nav`]: tree and board movement, jump to the next needs-you item.
//! - [`intent`]: key to intent mappings for tree focus and the overlay.
//! - [`forms`]: new plan, new task, message and note forms as state machines.
//! - [`hit`]: mouse (x, y) to tree row, board card or inbox item.
//! - [`draw`]: ratatui rendering of the tree, strip, board, overlay and forms.

pub mod draw;
pub mod forms;
pub mod hit;
pub mod intent;
pub mod marks;
pub mod nav;
pub mod remote;
pub mod remote_term;
pub mod rows;
pub mod state;
pub mod theme;
pub mod windows;

#[cfg(test)]
pub(crate) mod fixture;
