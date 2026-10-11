//! Pane layout of a window: a binary split tree, like tmux layout cells.
//!
//! A horizontal split puts its children side by side with a one-column
//! divider between them (tmux `split-window -h`). A vertical split stacks
//! them; the header row of the lower pane serves as the divider.

use ratatui::layout::Rect;

use crate::keys::Dir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Side by side.
    Horizontal,
    /// Stacked.
    Vertical,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Leaf(u64),
    /// `ratio` is the share of the first child.
    Split { axis: Axis, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// A split boundary on screen, for mouse resizing. `path` leads from the root
/// to the split (false = first child, true = second child). `area` is the
/// whole split, `at` the divider column (horizontal) or row (vertical).
#[derive(Debug, Clone)]
pub struct Divider {
    pub path: Vec<bool>,
    pub axis: Axis,
    pub area: Rect,
    pub at: u16,
}

/// Preset layouts, as in tmux `select-layout`.
pub const PRESETS: [&str; 5] = ["even-horizontal", "even-vertical", "main-vertical", "main-horizontal", "tiled"];

/// Smallest pane a resize may leave: a header row plus one line, two columns.
const MIN_ROWS: u16 = 2;
const MIN_COLS: u16 = 2;

fn split_rect(area: Rect, axis: Axis, ratio: f32) -> (Rect, Rect) {
    // Size of the first child: at least one cell for each child when possible.
    let first = |avail: u16| if avail < 2 { avail } else { ((avail as f32 * ratio).round() as u16).clamp(1, avail - 1) };
    match axis {
        Axis::Horizontal => {
            let avail = area.width.saturating_sub(1);
            let aw = first(avail);
            (Rect { width: aw, ..area }, Rect { x: area.x + aw + 1, width: avail - aw, ..area })
        }
        Axis::Vertical => {
            let ah = first(area.height);
            (Rect { height: ah, ..area }, Rect { y: area.y + ah, height: area.height - ah, ..area })
        }
    }
}

impl Node {
    /// Pane ids in layout order: left to right, top to bottom.
    pub fn leaves(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<u64>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.collect(out);
                b.collect(out);
            }
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        match self {
            Node::Leaf(x) => *x == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// The area of each pane inside `area`, in layout order.
    pub fn layout(&self, area: Rect) -> Vec<(u64, Rect)> {
        let mut out = Vec::new();
        self.place(area, &mut out);
        out
    }

    fn place(&self, area: Rect, out: &mut Vec<(u64, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, area)),
            Node::Split { axis, ratio, a, b } => {
                let (ra, rb) = split_rect(area, *axis, *ratio);
                a.place(ra, out);
                b.place(rb, out);
            }
        }
    }

    pub fn dividers(&self, area: Rect) -> Vec<Divider> {
        let mut out = Vec::new();
        self.find_dividers(area, &mut Vec::new(), &mut out);
        out
    }

    fn find_dividers(&self, area: Rect, path: &mut Vec<bool>, out: &mut Vec<Divider>) {
        let Node::Split { axis, ratio, a, b } = self else { return };
        let (ra, rb) = split_rect(area, *axis, *ratio);
        let at = match axis {
            Axis::Horizontal => ra.x + ra.width,
            Axis::Vertical => rb.y,
        };
        out.push(Divider { path: path.clone(), axis: *axis, area, at });
        path.push(false);
        a.find_dividers(ra, path, out);
        path.pop();
        path.push(true);
        b.find_dividers(rb, path, out);
        path.pop();
    }

    /// Split the pane `target` in two. The new pane `id` goes second.
    pub fn split(&mut self, target: u64, id: u64, axis: Axis) -> bool {
        match self {
            Node::Leaf(x) if *x == target => {
                *self = Node::Split { axis, ratio: 0.5, a: Box::new(Node::Leaf(target)), b: Box::new(Node::Leaf(id)) };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, id, axis) || b.split(target, id, axis),
        }
    }

    /// Remove the pane `id`. Its sibling takes the space of the parent split.
    /// The last pane cannot be removed.
    pub fn remove(&mut self, id: u64) -> bool {
        let Node::Split { a, b, .. } = self else { return false };
        if **a == Node::Leaf(id) {
            *self = std::mem::replace(b.as_mut(), Node::Leaf(0));
            return true;
        }
        if **b == Node::Leaf(id) {
            *self = std::mem::replace(a.as_mut(), Node::Leaf(0));
            return true;
        }
        a.remove(id) || b.remove(id)
    }

    /// Exchange the positions of two panes.
    pub fn swap(&mut self, x: u64, y: u64) {
        match self {
            Node::Leaf(id) if *id == x => *id = y,
            Node::Leaf(id) if *id == y => *id = x,
            Node::Leaf(_) => {}
            Node::Split { a, b, .. } => {
                a.swap(x, y);
                b.swap(x, y);
            }
        }
    }

    /// Move the nearest divider of pane `id` on `axis` by `delta` cells, like
    /// tmux `resize-pane -L/-R/-U/-D`. Returns false when no divider exists.
    pub fn resize(&mut self, id: u64, axis: Axis, delta: i32, area: Rect) -> bool {
        let Node::Split { axis: ax, ratio, a, b } = self else { return false };
        let (ra, rb) = split_rect(area, *ax, *ratio);
        let inner = if a.contains(id) { a.resize(id, axis, delta, ra) } else { b.contains(id) && b.resize(id, axis, delta, rb) };
        if inner {
            return true;
        }
        if *ax != axis || !self.contains(id) {
            return false;
        }
        let Node::Split { ratio, .. } = self else { unreachable!() };
        let (cur, avail, min) = match axis {
            Axis::Horizontal => (ra.width, area.width.saturating_sub(1), MIN_COLS),
            Axis::Vertical => (ra.height, area.height, MIN_ROWS),
        };
        if avail < 2 * min {
            return true;
        }
        let new = (cur as i32 + delta).clamp(min as i32, (avail - min) as i32);
        *ratio = new as f32 / avail as f32;
        true
    }

    /// Set the divider of the split at `path` to `pos`, a screen column or row.
    pub fn drag(&mut self, path: &[bool], pos: u16, area: Rect) {
        let Node::Split { axis, ratio, a, b } = self else { return };
        let (ra, rb) = split_rect(area, *axis, *ratio);
        match path.split_first() {
            Some((false, rest)) => a.drag(rest, pos, ra),
            Some((true, rest)) => b.drag(rest, pos, rb),
            None => {
                let (start, avail, min) = match axis {
                    Axis::Horizontal => (area.x, area.width.saturating_sub(1), MIN_COLS),
                    Axis::Vertical => (area.y, area.height, MIN_ROWS),
                };
                if avail >= 2 * min {
                    let new = pos.saturating_sub(start).clamp(min, avail - min);
                    *ratio = new as f32 / avail as f32;
                }
            }
        }
    }

    /// Build a preset layout from `ids` (in order). `None` for an unknown name.
    pub fn preset(name: &str, ids: &[u64]) -> Option<Node> {
        let (first, rest) = ids.split_first()?;
        if rest.is_empty() {
            return Some(Node::Leaf(*first));
        }
        let main = |axis, other| Node::Split {
            axis,
            ratio: 0.6,
            a: Box::new(Node::Leaf(*first)),
            b: Box::new(chain(rest, other)),
        };
        Some(match name {
            "even-horizontal" => chain(ids, Axis::Horizontal),
            "even-vertical" => chain(ids, Axis::Vertical),
            "main-vertical" => main(Axis::Horizontal, Axis::Vertical),
            "main-horizontal" => main(Axis::Vertical, Axis::Horizontal),
            "tiled" => {
                let cols = (ids.len() as f32).sqrt().ceil() as usize;
                let rows: Vec<Node> = ids.chunks(cols).map(|r| chain(r, Axis::Horizontal)).collect();
                chain_nodes(rows, Axis::Vertical)
            }
            _ => return None,
        })
    }
}

/// Equal-sized panes along `axis`.
fn chain(ids: &[u64], axis: Axis) -> Node {
    chain_nodes(ids.iter().map(|&id| Node::Leaf(id)).collect(), axis)
}

fn chain_nodes(mut nodes: Vec<Node>, axis: Axis) -> Node {
    let n = nodes.len();
    let first = nodes.remove(0);
    if n == 1 {
        return first;
    }
    Node::Split { axis, ratio: 1.0 / n as f32, a: Box::new(first), b: Box::new(chain_nodes(nodes, axis)) }
}

/// The pane next to `from` in direction `dir`. Prefers the pane with the most
/// shared edge, like tmux.
pub fn neighbor(rects: &[(u64, Rect)], from: u64, dir: Dir) -> Option<u64> {
    let (_, f) = *rects.iter().find(|(id, _)| *id == from)?;
    let overlap = |a0: u16, a1: u16, b0: u16, b1: u16| a1.min(b1).saturating_sub(a0.max(b0));
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter_map(|&(id, r)| {
            let (gap, shared) = match dir {
                Dir::Left => (f.x as i32 - r.right() as i32, overlap(f.y, f.bottom(), r.y, r.bottom())),
                Dir::Right => (r.x as i32 - f.right() as i32, overlap(f.y, f.bottom(), r.y, r.bottom())),
                Dir::Up => (f.y as i32 - r.bottom() as i32, overlap(f.x, f.right(), r.x, r.right())),
                Dir::Down => (r.y as i32 - f.bottom() as i32, overlap(f.x, f.right(), r.x, r.right())),
            };
            (gap >= 0 && shared > 0).then_some((gap, std::cmp::Reverse(shared), id))
        })
        .min()
        .map(|(_, _, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0, y: 1, width: 81, height: 40 };

    #[test]
    fn split_and_remove() {
        let mut n = Node::Leaf(1);
        assert!(n.split(1, 2, Axis::Horizontal));
        assert!(n.split(2, 3, Axis::Vertical));
        assert_eq!(n.leaves(), vec![1, 2, 3]);
        let l = n.layout(AREA);
        assert_eq!(l[0].1, Rect { x: 0, y: 1, width: 40, height: 40 });
        assert_eq!(l[1].1, Rect { x: 41, y: 1, width: 40, height: 20 });
        assert_eq!(l[2].1, Rect { x: 41, y: 21, width: 40, height: 20 });
        assert!(n.remove(2));
        assert_eq!(n.layout(AREA)[1].1, Rect { x: 41, y: 1, width: 40, height: 40 });
        assert!(n.remove(1));
        assert_eq!(n, Node::Leaf(3));
        assert!(!n.remove(3));
    }

    #[test]
    fn neighbors() {
        let mut n = Node::Leaf(1);
        n.split(1, 2, Axis::Horizontal);
        n.split(2, 3, Axis::Vertical);
        let l = n.layout(AREA);
        assert_eq!(neighbor(&l, 1, Dir::Right), Some(2));
        assert_eq!(neighbor(&l, 3, Dir::Left), Some(1));
        assert_eq!(neighbor(&l, 3, Dir::Up), Some(2));
        assert_eq!(neighbor(&l, 2, Dir::Down), Some(3));
        assert_eq!(neighbor(&l, 1, Dir::Left), None);
    }

    #[test]
    fn resize_and_drag() {
        let mut n = Node::Leaf(1);
        n.split(1, 2, Axis::Horizontal);
        assert!(n.resize(2, Axis::Horizontal, -10, AREA));
        assert_eq!(n.layout(AREA)[0].1.width, 30);
        assert!(!n.resize(2, Axis::Vertical, 5, AREA));
        let d = &n.dividers(AREA)[0];
        assert_eq!(d.at, 30);
        n.drag(&d.path.clone(), 60, AREA);
        assert_eq!(n.layout(AREA)[0].1.width, 60);
    }

    #[test]
    fn presets() {
        let t = Node::preset("tiled", &[1, 2, 3, 4]).unwrap();
        let l = t.layout(AREA);
        assert_eq!(l.iter().map(|(_, r)| (r.width, r.height)).collect::<Vec<_>>(), vec![(40, 20); 4]);
        let e = Node::preset("even-horizontal", &[1, 2, 3]).unwrap();
        let w: Vec<u16> = e.layout(Rect { width: 92, ..AREA }).iter().map(|(_, r)| r.width).collect();
        assert_eq!(w, vec![30, 30, 30]);
        assert_eq!(Node::preset("nope", &[1, 2]), None);
    }
}
