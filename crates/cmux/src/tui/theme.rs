//! Colours (UI design 2.1) and glyph characters.

use cmux_core::glyph::Glyph;
use cmux_proto::view::{PlanState, TaskState};
use ratatui::style::Color;

pub const AMBER: Color = Color::Rgb(0xc6, 0x81, 0x08);
pub const PURPLE: Color = Color::Rgb(0xa6, 0x62, 0xea);
pub const RED: Color = Color::Rgb(0xe2, 0x50, 0x50);
pub const GREEN: Color = Color::Rgb(0x29, 0xa4, 0x6a);
pub const GREY: Color = Color::Rgb(0x8f, 0x8f, 0x97);
pub const BLUE: Color = Color::Rgb(0x60, 0x92, 0xdd);
pub const ACCENT: Color = Color::Rgb(0xd9, 0x77, 0x57);
pub const SELECT_BG: Color = Color::Rgb(0x3a, 0x3a, 0x46);

const SPINNER: [char; 10] = ['⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '⠋', '⠙'];

/// The glyph's character; `frame` animates the working spinner.
pub fn glyph_char(g: Glyph, frame: usize) -> char {
    match g {
        Glyph::Working => SPINNER[frame % SPINNER.len()],
        other => other.as_char(),
    }
}

pub fn glyph_color(g: Glyph) -> Color {
    match g {
        Glyph::Working | Glyph::Stalled => AMBER,
        Glyph::Idle | Glyph::Parked => GREY,
        Glyph::NeedsYou => PURPLE,
        Glyph::Unseen => BLUE,
        Glyph::Success => GREEN,
        Glyph::Failed => RED,
    }
}

pub fn task_state_color(s: TaskState) -> Color {
    match s {
        TaskState::Todo | TaskState::Cancelled => GREY,
        TaskState::InProgress | TaskState::Testing => AMBER,
        TaskState::Passing | TaskState::Merged => GREEN,
        TaskState::Failing | TaskState::Blocked => RED,
    }
}

pub fn plan_state_color(s: PlanState) -> Color {
    match s {
        PlanState::Draft | PlanState::Cancelled => GREY,
        PlanState::Proposed => BLUE,
        PlanState::Accepted => AMBER,
        PlanState::Done => GREEN,
    }
}
