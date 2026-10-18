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
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
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
pub const PRESETS: [&str; 5] = [
    "even-horizontal",
    "even-vertical",
    "main-vertical",
    "main-horizontal",
    "tiled",
];

/// Smallest pane a resize may leave: a header row plus one line, two columns.
const MIN_ROWS: u16 = 2;
const MIN_COLS: u16 = 2;

fn split_rect(area: Rect, axis: Axis, ratio: f32) -> (Rect, Rect) {
    // Size of the first child: at least one cell for each child when possible.
    let first = |avail: u16| {
        if avail < 2 {
            avail
        } else {
            ((avail as f32 * ratio).round() as u16).clamp(1, avail - 1)
        }
    };
    match axis {
        Axis::Horizontal => {
            let avail = area.width.saturating_sub(1);
            let aw = first(avail);
            (
                Rect { width: aw, ..area },
                Rect {
                    x: area.x + aw + 1,
                    width: avail - aw,
                    ..area
                },
            )
        }
        Axis::Vertical => {
            let ah = first(area.height);
            (
                Rect { height: ah, ..area },
                Rect {
                    y: area.y + ah,
                    height: area.height - ah,
                    ..area
                },
            )
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
        let Node::Split { axis, ratio, a, b } = self else {
            return;
        };
        let (ra, rb) = split_rect(area, *axis, *ratio);
        let at = match axis {
            Axis::Horizontal => ra.x + ra.width,
            Axis::Vertical => rb.y,
        };
        out.push(Divider {
            path: path.clone(),
            axis: *axis,
            area,
            at,
        });
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
                *self = Node::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(id)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, id, axis) || b.split(target, id, axis),
        }
    }

    /// Remove the pane `id`. Its sibling takes the space of the parent split.
    /// The last pane cannot be removed.
    pub fn remove(&mut self, id: u64) -> bool {
        let Node::Split { a, b, .. } = self else {
            return false;
        };
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
        let Node::Split {
            axis: ax,
            ratio,
            a,
            b,
        } = self
        else {
            return false;
        };
        let (ra, rb) = split_rect(area, *ax, *ratio);
        let inner = if a.contains(id) {
            a.resize(id, axis, delta, ra)
        } else {
            b.contains(id) && b.resize(id, axis, delta, rb)
        };
        if inner {
            return true;
        }
        if *ax != axis || !self.contains(id) {
            return false;
        }
        let Node::Split { ratio, .. } = self else {
            unreachable!()
        };
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
        let Node::Split { axis, ratio, a, b } = self else {
            return;
        };
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
                let rows: Vec<Node> = ids
                    .chunks(cols)
                    .map(|r| chain(r, Axis::Horizontal))
                    .collect();
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
    Node::Split {
        axis,
        ratio: 1.0 / n as f32,
        a: Box::new(first),
        b: Box::new(chain_nodes(nodes, axis)),
    }
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
                Dir::Left => (
                    f.x as i32 - r.right() as i32,
                    overlap(f.y, f.bottom(), r.y, r.bottom()),
                ),
                Dir::Right => (
                    r.x as i32 - f.right() as i32,
                    overlap(f.y, f.bottom(), r.y, r.bottom()),
                ),
                Dir::Up => (
                    f.y as i32 - r.bottom() as i32,
                    overlap(f.x, f.right(), r.x, r.right()),
                ),
                Dir::Down => (
                    r.y as i32 - f.bottom() as i32,
                    overlap(f.x, f.right(), r.x, r.right()),
                ),
            };
            (gap >= 0 && shared > 0).then_some((gap, std::cmp::Reverse(shared), id))
        })
        .min()
        .map(|(_, _, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 1,
        width: 81,
        height: 40,
    };

    #[test]
    fn split_and_remove() {
        let mut n = Node::Leaf(1);
        assert!(n.split(1, 2, Axis::Horizontal));
        assert!(n.split(2, 3, Axis::Vertical));
        assert_eq!(n.leaves(), vec![1, 2, 3]);
        let l = n.layout(AREA);
        assert_eq!(
            l[0].1,
            Rect {
                x: 0,
                y: 1,
                width: 40,
                height: 40
            }
        );
        assert_eq!(
            l[1].1,
            Rect {
                x: 41,
                y: 1,
                width: 40,
                height: 20
            }
        );
        assert_eq!(
            l[2].1,
            Rect {
                x: 41,
                y: 21,
                width: 40,
                height: 20
            }
        );
        assert!(n.remove(2));
        assert_eq!(
            n.layout(AREA)[1].1,
            Rect {
                x: 41,
                y: 1,
                width: 40,
                height: 40
            }
        );
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
        assert_eq!(
            l.iter()
                .map(|(_, r)| (r.width, r.height))
                .collect::<Vec<_>>(),
            vec![(40, 20); 4]
        );
        let e = Node::preset("even-horizontal", &[1, 2, 3]).unwrap();
        let w: Vec<u16> = e
            .layout(Rect { width: 92, ..AREA })
            .iter()
            .map(|(_, r)| r.width)
            .collect();
        assert_eq!(w, vec![30, 30, 30]);
        assert_eq!(Node::preset("nope", &[1, 2]), None);
    }

    /// Checks that the panes and the horizontal dividers cover `area` exactly
    /// once: no pane overlaps another, nothing is left empty, and nothing
    /// falls outside `area`. A vertical divider is the header row of the
    /// lower pane, so it is already counted in that pane.
    fn assert_tiles(node: &Node, area: Rect) {
        let width = area.width as usize;
        let mut cover = vec![0u32; width * area.height as usize];
        let mut mark = |x: u16, y: u16| {
            assert!(
                x >= area.x && x < area.right() && y >= area.y && y < area.bottom(),
                "cell ({x},{y}) outside {area:?}"
            );
            cover[(y - area.y) as usize * width + (x - area.x) as usize] += 1;
        };
        for (_, r) in node.layout(area) {
            for y in r.y..r.bottom() {
                for x in r.x..r.right() {
                    mark(x, y);
                }
            }
        }
        for d in node.dividers(area) {
            if d.axis == Axis::Horizontal {
                for y in d.area.y..d.area.bottom() {
                    mark(d.at, y);
                }
            }
        }
        assert!(
            cover.iter().all(|&c| c == 1),
            "cells not covered exactly once in {area:?}: {cover:?}"
        );
    }

    #[test]
    fn single_pane_fills_area() {
        let n = Node::Leaf(7);
        assert_eq!(n.layout(AREA), vec![(7, AREA)]);
        assert_tiles(&n, AREA);
    }

    #[test]
    fn split_sizes_add_up() {
        for ratio in [0.1, 0.25, 0.5, 0.6, 0.9] {
            let h = Node::Split {
                axis: Axis::Horizontal,
                ratio,
                a: Box::new(Node::Leaf(1)),
                b: Box::new(Node::Leaf(2)),
            };
            let l = h.layout(AREA);
            let (a, b) = (l[0].1, l[1].1);
            assert_eq!((a.y, a.height), (AREA.y, AREA.height));
            assert_eq!((b.y, b.height), (AREA.y, AREA.height));
            assert_eq!(a.x, AREA.x);
            // One column is the divider between the two panes.
            assert_eq!(a.width + 1 + b.width, AREA.width, "ratio {ratio}");
            assert_eq!(b.x, a.right() + 1, "ratio {ratio}");
            assert_eq!(b.right(), AREA.right(), "ratio {ratio}");
            assert_tiles(&h, AREA);

            let v = Node::Split {
                axis: Axis::Vertical,
                ratio,
                a: Box::new(Node::Leaf(1)),
                b: Box::new(Node::Leaf(2)),
            };
            let l = v.layout(AREA);
            let (a, b) = (l[0].1, l[1].1);
            assert_eq!((a.x, a.width), (AREA.x, AREA.width));
            assert_eq!((b.x, b.width), (AREA.x, AREA.width));
            // No divider row: the lower pane's header is the divider.
            assert_eq!(a.height + b.height, AREA.height, "ratio {ratio}");
            assert_eq!(b.y, a.bottom(), "ratio {ratio}");
            assert_eq!(b.bottom(), AREA.bottom(), "ratio {ratio}");
            assert_tiles(&v, AREA);
        }
    }

    #[test]
    fn split_tree_geometry() {
        let mut n = Node::Leaf(1);
        n.split(1, 2, Axis::Horizontal);
        n.split(2, 3, Axis::Vertical);
        n.split(3, 4, Axis::Horizontal);
        let l = n.layout(AREA);
        assert_eq!(n.leaves(), vec![1, 2, 3, 4]);
        assert_eq!(
            l[0].1,
            Rect {
                x: 0,
                y: 1,
                width: 40,
                height: 40
            }
        );
        assert_eq!(
            l[1].1,
            Rect {
                x: 41,
                y: 1,
                width: 40,
                height: 20
            }
        );
        // 39 usable columns split 20 / 19 around a divider at column 61.
        assert_eq!(
            l[2].1,
            Rect {
                x: 41,
                y: 21,
                width: 20,
                height: 20
            }
        );
        assert_eq!(
            l[3].1,
            Rect {
                x: 62,
                y: 21,
                width: 19,
                height: 20
            }
        );
        assert_tiles(&n, AREA);
    }

    #[test]
    fn presets_with_few_panes() {
        for name in PRESETS {
            for n in 1..=4u64 {
                let ids: Vec<u64> = (1..=n).collect();
                let node =
                    Node::preset(name, &ids).unwrap_or_else(|| panic!("{name} with {n} panes"));
                assert_eq!(node.leaves(), ids, "{name} with {n} panes");
                assert_eq!(node.layout(AREA).len(), n as usize, "{name} with {n} panes");
                assert_tiles(&node, AREA);
            }
        }
    }

    #[test]
    fn preset_geometry_small() {
        let main = Node::preset("main-vertical", &[1, 2, 3])
            .unwrap()
            .layout(AREA);
        assert_eq!(
            main[0].1,
            Rect {
                x: 0,
                y: 1,
                width: 48,
                height: 40
            }
        );
        assert_eq!(
            main[1].1,
            Rect {
                x: 49,
                y: 1,
                width: 32,
                height: 20
            }
        );
        assert_eq!(
            main[2].1,
            Rect {
                x: 49,
                y: 21,
                width: 32,
                height: 20
            }
        );

        let main = Node::preset("main-horizontal", &[1, 2, 3])
            .unwrap()
            .layout(AREA);
        assert_eq!(
            main[0].1,
            Rect {
                x: 0,
                y: 1,
                width: 81,
                height: 24
            }
        );
        assert_eq!(
            main[1].1,
            Rect {
                x: 0,
                y: 25,
                width: 40,
                height: 16
            }
        );
        assert_eq!(
            main[2].1,
            Rect {
                x: 41,
                y: 25,
                width: 40,
                height: 16
            }
        );

        // Three panes: two on the first row, one spanning the second.
        let tiled = Node::preset("tiled", &[1, 2, 3]).unwrap().layout(AREA);
        assert_eq!(
            tiled[0].1,
            Rect {
                x: 0,
                y: 1,
                width: 40,
                height: 20
            }
        );
        assert_eq!(
            tiled[1].1,
            Rect {
                x: 41,
                y: 1,
                width: 40,
                height: 20
            }
        );
        assert_eq!(
            tiled[2].1,
            Rect {
                x: 0,
                y: 21,
                width: 81,
                height: 20
            }
        );
    }

    #[test]
    fn preset_single_pane_fills_area() {
        for name in PRESETS {
            assert_eq!(
                Node::preset(name, &[9]).unwrap().layout(AREA),
                vec![(9, AREA)],
                "{name}"
            );
        }
        assert_eq!(Node::preset("tiled", &[]), None);
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        for width in 0..=3 {
            for height in 0..=3 {
                let area = Rect {
                    x: 5,
                    y: 5,
                    width,
                    height,
                };
                for name in PRESETS {
                    for n in 1..=4u64 {
                        let ids: Vec<u64> = (1..=n).collect();
                        let node = Node::preset(name, &ids).unwrap();
                        let panes = node.layout(area);
                        assert_eq!(panes.len(), n as usize);
                        for (_, r) in panes {
                            assert!(
                                r.width <= width && r.height <= height,
                                "{name} {n} in {area:?}: {r:?}"
                            );
                        }
                        node.dividers(area);
                    }
                }
                let mut n = Node::Leaf(1);
                n.split(1, 2, Axis::Horizontal);
                n.split(2, 3, Axis::Vertical);
                assert_eq!(n.layout(area).len(), 3);
                n.drag(&[false], 0, area);
                n.drag(&[true], 100, area);
            }
        }
    }
}
