//! Shared selection, ordering and drop feedback for numbered slot lists.
//! Views own their data and undo; these operations only rearrange slot identities.

use super::theme;
use egui::{Modifiers, Rect, Stroke};
use std::collections::{BTreeSet, HashSet};

pub fn click(
    sel: &mut BTreeSet<usize>,
    cursor: &mut Option<usize>,
    slot: usize,
    m: Modifiers,
    first: usize,
) {
    if m.shift {
        let a = cursor.unwrap_or(slot);
        if !m.command {
            sel.clear();
        }
        sel.extend(a.min(slot)..=a.max(slot));
    } else if m.command {
        if !sel.remove(&slot) {
            sel.insert(slot);
        }
        *cursor = Some(slot);
    } else {
        sel.clear();
        sel.insert(slot);
        *cursor = Some(slot);
    }
    sel.retain(|s| *s >= first);
}

/// Insert selected cells at the boundary `to`, closing up their old positions.
pub fn insert<T: Clone>(order: &mut Vec<T>, sel: &[usize], to: usize, first: usize) {
    if sel.is_empty() {
        return;
    }
    let to = to.clamp(first, order.len());
    let moved: Vec<T> = sel.iter().map(|&s| order[s].clone()).collect();
    let set: HashSet<usize> = sel.iter().copied().collect();
    let mut rest: Vec<T> = order
        .iter()
        .enumerate()
        .filter(|(i, _)| !set.contains(i))
        .map(|(_, o)| o.clone())
        .collect();
    let at = to - sel.iter().filter(|&&s| s < to).count();
    rest.splice(at..at, moved);
    *order = rest;
}

/// Place a group in consecutive cells, exchanging displaced cells with the freed ones.
/// Returns the actual first target (clamped so the entire group fits).
pub fn place<T: Clone>(order: &mut [T], sel: &[usize], to: usize, first: usize) -> usize {
    if sel.is_empty() {
        return to;
    }
    let to = to.clamp(first, order.len() - sel.len());
    let targets: Vec<usize> = (to..to + sel.len()).collect();
    let tset: HashSet<usize> = targets.iter().copied().collect();
    let sset: HashSet<usize> = sel.iter().copied().collect();
    let displaced: Vec<usize> = targets
        .iter()
        .copied()
        .filter(|t| !sset.contains(t))
        .collect();
    let freed: Vec<usize> = sel.iter().copied().filter(|s| !tset.contains(s)).collect();
    let old = order.to_vec();
    for (&s, &t) in sel.iter().zip(&targets) {
        order[t] = old[s].clone();
    }
    for (&d, &f) in displaced.iter().zip(&freed) {
        order[f] = old[d].clone();
    }
    to
}

/// Puts a group at `to, to+1, …` like an insert: what's under it moves
/// down, pushing what's below further down until enough empty slots (the
/// ones the group left included) have taken up the shift. Nothing below
/// that changes. Returns the last slot that changed, or `None` (order left
/// alone) when there isn't enough empty room below.
pub fn shift_place<T: Clone>(
    order: &mut [T],
    sel: &[usize],
    to: usize,
    first: usize,
    free: impl Fn(&T) -> bool,
) -> Option<usize> {
    if sel.is_empty() || sel.len() > order.len().saturating_sub(first) {
        return None;
    }
    let to = to.clamp(first, order.len() - sel.len());
    let mut cells: Vec<Option<T>> = order.iter().cloned().map(Some).collect();
    let moved: Vec<T> = sel.iter().map(|&s| cells[s].take().expect("selected once")).collect();
    // Empties pushed out of the way, to refill the holes the group left.
    let mut spare: Vec<T> = Vec::new();
    let mut carry: std::collections::VecDeque<T> = std::collections::VecDeque::new();
    let mut moved = moved.into_iter();
    let mut last = to;
    let mut i = to;
    loop {
        let incoming = match moved.next() {
            Some(m) => m,
            None => match carry.pop_front() {
                Some(c) => c,
                None => break,
            },
        };
        if i >= cells.len() {
            return None;
        }
        match cells[i].replace(incoming) {
            Some(old) if free(&old) => spare.push(old),
            Some(old) => carry.push_back(old),
            None => {}
        }
        last = i;
        i += 1;
    }
    let mut spare = spare.into_iter();
    for c in cells.iter_mut().filter(|c| c.is_none()) {
        *c = spare.next();
    }
    for (o, c) in order.iter_mut().zip(cells) {
        *o = c.expect("every hole refilled");
    }
    Some(last.max(sel.last().copied().unwrap_or(0)))
}

pub fn nudge<T>(order: &mut [T], sel: &[usize], dir: i32, first: usize) {
    let (Some(&lo), Some(&hi)) = (sel.first(), sel.last()) else {
        return;
    };
    if (dir < 0 && lo <= first) || (dir > 0 && hi + 1 >= order.len()) {
        return;
    }
    if dir < 0 {
        for &s in sel {
            order.swap(s - 1, s);
        }
    } else {
        for &s in sel.iter().rev() {
            order.swap(s, s + 1);
        }
    }
}

pub fn insert_empty<T>(
    order: &mut [T],
    at: usize,
    n: usize,
    free: impl Fn(&T) -> bool,
) -> (usize, usize) {
    let (mut made, mut last) = (0, at);
    while made < n {
        let from = at + made.max(1);
        let Some(e) = (from..order.len()).find(|&i| free(&order[i])) else {
            break;
        };
        order[at..=e].rotate_right(1);
        made += 1;
        last = last.max(e);
    }
    (made, last)
}

pub fn close_gap<T>(order: &mut [T], at: usize, free: impl Fn(&T) -> bool) {
    let end = (at + 1..order.len())
        .find(|&i| free(&order[i]))
        .unwrap_or(order.len());
    order[at..end].rotate_left(1);
}

#[derive(Clone, Copy)]
pub enum DropTarget {
    /// Swap in place: the block lands at the slot, what's there goes to the
    /// slots it left.
    Place(usize),
    /// Insert at a row boundary; everything between old and new place shifts.
    Insert(usize),
    /// Land at the slot like `Place`, but push what's there down until the
    /// next empty slots take it up (`shift_place`).
    Shift(usize),
}

/// What a plain drop does in a list (⌥/⌃ always swaps in place instead).
#[derive(Clone, Copy, PartialEq)]
pub enum PlainDrop {
    Insert,
    Shift,
}

/// Where in a drag the grabbed row sat: selected cells before it (the
/// selection packs up when placed) and how far down its row the press was
/// (0..1).
#[derive(Clone, Copy, Default)]
pub struct Grab {
    pub before: usize,
    pub frac: f32,
}

impl Grab {
    /// `frac`: how far down the grabbed row the press was.
    pub fn new(sel: &BTreeSet<usize>, slot: usize, frac: f32) -> Self {
        Self {
            before: sel.range(..slot).count(),
            frac: frac.clamp(0.0, 0.999),
        }
    }
}

/// What differs between slot lists when dragging. Everything else (where a
/// block lands, the feedback, the note) is shared.
pub struct DropRules {
    /// Lowest slot a drop may use (the sample pool's 000 is "off").
    pub first: usize,
    /// Slots in the whole list, across banks.
    pub len: usize,
    /// What a plain drop does; ⌥/⌃ swaps in place. Presets insert (they're
    /// copies, order is free); samples push down to the next empty slot.
    pub plain: PlainDrop,
}

/// The row under a drag. Rows are `pitch` apart on screen (height plus any
/// spacing, so the gaps count too); `slot_of(row)` maps a visible row to
/// its slot, and `slot_of(rows)` is the slot after the last row. Lists that
/// hide rows (filters) map through it; full lists are `first + row`.
pub struct RowHit<'a> {
    pub rect: Rect,
    pub pitch: f32,
    pub row: usize,
    pub slot_of: &'a dyn Fn(usize) -> usize,
}

/// Where a drag over `hit` lands. A swap drop places the block the way it's
/// held: the grabbed row lands under the pointer and snaps to the nearest
/// row. An insert goes to the row boundary nearest the pointer.
pub fn drop_target_rows(ui: &egui::Ui, hit: &RowHit, n: usize, grab: Grab, rules: &DropRules) -> DropTarget {
    let (flip, y) = ui.input(|i| {
        (
            i.modifiers.ctrl || i.modifiers.alt,
            i.pointer.interact_pos().map(|p| p.y),
        )
    });
    let frac = y.map_or(0.5, |y| (y - hit.rect.top()) / hit.pitch);
    if !flip && rules.plain == PlainDrop::Insert {
        let at = (hit.slot_of)(hit.row + usize::from(frac > 0.5));
        return DropTarget::Insert(at.clamp(rules.first, rules.len));
    }
    let row = (hit.row as isize + (frac - grab.frac).round() as isize).max(0) as usize;
    let top = (hit.slot_of)(row).saturating_sub(grab.before);
    let last = rules.len.saturating_sub(n.max(1)).max(rules.first);
    let top = top.clamp(rules.first, last);
    if flip { DropTarget::Place(top) } else { DropTarget::Shift(top) }
}

/// The slots a target's feedback covers: the block's new place, or for an
/// insert the `n` slots after the line.
pub fn landing_span(target: DropTarget, n: usize) -> std::ops::Range<usize> {
    let at = match target {
        DropTarget::Place(at) | DropTarget::Insert(at) | DropTarget::Shift(at) => at,
    };
    at..at + n.max(1)
}

/// The slot order a drop would make (`order[new] = old`), to count what it
/// touches before it happens. `free(slot)`: the slot is empty now.
pub fn preview(len: usize, sel: &[usize], target: DropTarget, first: usize, free: impl Fn(usize) -> bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    match target {
        DropTarget::Place(at) => {
            place(&mut order, sel, at, first);
        }
        DropTarget::Insert(at) => insert(&mut order, sel, at, first),
        DropTarget::Shift(at) => {
            if shift_place(&mut order, sel, at, first, |&i| free(i)).is_none() {
                insert(&mut order, sel, at, first);
            }
        }
    }
    order
}

/// The pointer note for a drop: what lands where, what else moves, and how
/// to get the other kind of drop. `occupied` says a slot holds something.
pub fn landing_note(
    target: DropTarget,
    sel: &BTreeSet<usize>,
    rules: &DropRules,
    label: impl Fn(usize) -> String,
    occupied: impl Fn(usize) -> bool,
) -> String {
    let n = sel.len().max(1);
    let other = match target {
        DropTarget::Place(_) => match rules.plain {
            PlainDrop::Insert => "Release ⌥ to insert instead",
            PlainDrop::Shift => "Release ⌥ to push down instead",
        },
        _ => "⌥ swap in place instead",
    };
    let range = |top: usize| {
        if n == 1 { label(top) } else { format!("{}–{}", label(top), label(top + n - 1)) }
    };
    match target {
        DropTarget::Shift(top) => {
            if sel.iter().copied().eq(top..top + n) {
                return format!("Already at {}", range(top));
            }
            let len = rules.len;
            let sv: Vec<usize> = sel.iter().copied().collect();
            let mut order: Vec<usize> = (0..len).collect();
            match shift_place(&mut order, &sv, top, rules.first, |&i| !occupied(i)) {
                Some(_) => {
                    // Samples that end up somewhere else, apart from the dragged ones.
                    let pushed: Vec<usize> = order
                        .iter()
                        .enumerate()
                        .filter(|(i, o)| *i != **o && !sel.contains(o) && occupied(**o))
                        .map(|(i, _)| i)
                        .collect();
                    match (pushed.first(), pushed.last()) {
                        (Some(_), Some(&end)) => format!(
                            "Insert at {} · {} pushed down to {}\n{other}",
                            range(top),
                            pushed.len(),
                            label(end)
                        ),
                        _ => format!("Place at {}\n{other}", range(top)),
                    }
                }
                None => format!(
                    "Insert at {} · no empty slot below: slots in between shift\n{other}",
                    range(top)
                ),
            }
        }
        DropTarget::Place(top) => {
            let range = if n == 1 {
                label(top)
            } else {
                format!("{}–{}", label(top), label(top + n - 1))
            };
            if sel.iter().copied().eq(top..top + n) {
                return format!("Already at {range}");
            }
            let swaps = (top..top + n).filter(|t| !sel.contains(t) && occupied(*t)).count();
            if swaps > 0 {
                format!("Place at {range} · {swaps} swap into the freed slots\n{other}")
            } else {
                format!("Place at {range}\n{other}")
            }
        }
        DropTarget::Insert(at) => {
            let to = if at < rules.len { format!("before {}", label(at)) } else { "at the end".into() };
            format!("Insert {to} · slots in between shift\n{other}")
        }
    }
}

/// Landing feedback: a bright line at the first boundary the drop lands on
/// (`y.min`) and a tinted area down to `y.max`. Paint after the rows, so
/// row fills don't cover it. `deny` paints it as refused.
pub fn paint_landing(ui: &egui::Ui, x: egui::Rangef, y: egui::Rangef, target: DropTarget, deny: bool) {
    let c = if deny { theme::DENY } else { theme::DEVICE_ACCENT_BRIGHT };
    let area = Rect::from_x_y_ranges(x, y);
    let p = ui.painter();
    match target {
        DropTarget::Place(_) | DropTarget::Shift(_) => {
            p.rect_filled(area, 0.0, c.gamma_multiply(0.22));
            p.rect_stroke(area, 0.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
        }
        DropTarget::Insert(_) => {
            p.rect_filled(area, 0.0, c.gamma_multiply(0.10));
        }
    }
    // The insertion line, with end ticks so it reads as "between rows".
    let s = Stroke::new(2.5, c);
    p.hline(x, y.min, s);
    for x in [x.min, x.max] {
        p.vline(x, (y.min - 4.0)..=(y.min + 4.0), s);
    }
}

/// Scrolls a list while a drag hovers near its top or bottom edge, faster
/// the closer it gets. Call inside the scroll area.
pub fn drag_autoscroll(ui: &egui::Ui, edge: f32) {
    let view = ui.clip_rect();
    let Some(p) = ui.ctx().pointer_latest_pos().filter(|p| view.contains(*p)) else { return };
    let d = if p.y < view.top() + edge {
        view.top() + edge - p.y
    } else if p.y > view.bottom() - edge {
        view.bottom() - edge - p.y
    } else {
        return;
    };
    ui.scroll_with_delta_animation(egui::vec2(0.0, d * 0.35), egui::style::ScrollAnimation::none());
    ui.ctx().request_repaint();
}
