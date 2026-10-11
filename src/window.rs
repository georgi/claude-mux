//! A window (shown as a tab): one or more panes in a split layout.

use ratatui::layout::Rect;

use crate::layout::Node;
use crate::pane::Pane;

pub struct Window {
    pub id: u64,
    pub panes: Vec<Pane>,
    pub layout: Node,
    /// Id of the focused pane.
    pub active: u64,
    /// Id of the previously focused pane, for tmux `last-pane`.
    pub last: Option<u64>,
    pub zoomed: bool,
    pub custom_name: Option<String>,
    /// Index into `layout::PRESETS` of the last preset applied.
    pub preset: Option<usize>,
}

/// Where a pane is drawn. `header` is the title row shown when the window
/// shows more than one pane.
pub struct PaneArea {
    pub id: u64,
    pub header: Option<Rect>,
    pub body: Rect,
}

impl Window {
    pub fn new(id: u64, pane: Pane) -> Window {
        Window {
            id,
            layout: Node::Leaf(pane.id),
            active: pane.id,
            panes: vec![pane],
            last: None,
            zoomed: false,
            custom_name: None,
            preset: None,
        }
    }

    /// The focused pane. A window always has at least one pane.
    pub fn pane(&self) -> &Pane {
        self.get(self.active).unwrap_or(&self.panes[0])
    }

    pub fn get(&self, id: u64) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    /// Whether more than one pane is on screen.
    pub fn is_split(&self) -> bool {
        self.panes.len() > 1 && !self.zoomed
    }

    /// The visible panes and where they go inside `area`.
    pub fn areas(&self, area: Rect) -> Vec<PaneArea> {
        if !self.is_split() {
            return vec![PaneArea { id: self.active, header: None, body: area }];
        }
        self.layout
            .layout(area)
            .into_iter()
            .map(|(id, r)| PaneArea {
                id,
                header: Some(Rect { height: 1.min(r.height), ..r }),
                body: Rect { y: r.y + 1.min(r.height), height: r.height.saturating_sub(1), ..r },
            })
            .collect()
    }

    /// The pane under a screen cell, and whether the cell is on its header.
    pub fn pane_at(&self, area: Rect, col: u16, row: u16) -> Option<(u64, bool)> {
        let pos = ratatui::layout::Position { x: col, y: row };
        self.areas(area).into_iter().find_map(|a| {
            if a.header.is_some_and(|h| h.contains(pos)) {
                Some((a.id, true))
            } else {
                a.body.contains(pos).then_some((a.id, false))
            }
        })
    }

    pub fn focus(&mut self, id: u64) {
        if id != self.active && self.get(id).is_some() {
            self.last = Some(self.active);
            self.active = id;
        }
        if let Some(p) = self.get_mut(id) {
            p.attention = false;
        }
    }

    /// Take a pane out of the window. Focus moves to the last pane, else to
    /// the first one. Returns `None` for an unknown id.
    pub fn take(&mut self, id: u64) -> Option<Pane> {
        let i = self.panes.iter().position(|p| p.id == id)?;
        let pane = self.panes.remove(i);
        self.layout.remove(id);
        if self.panes.len() <= 1 {
            self.zoomed = false;
        }
        if self.last == Some(id) {
            self.last = None;
        }
        if self.active == id {
            let next = self.last.take().filter(|l| self.get(*l).is_some()).or_else(|| self.layout.leaves().first().copied());
            if let Some(next) = next {
                self.active = next;
                if let Some(p) = self.get_mut(next) {
                    p.attention = false;
                }
            }
        }
        Some(pane)
    }

    /// Tab name: the user-set name, else the focused pane's name.
    pub fn name(&self) -> String {
        self.custom_name.clone().unwrap_or_else(|| self.pane().name())
    }

    pub fn attention(&self) -> bool {
        self.panes.iter().any(|p| p.attention)
    }
}
