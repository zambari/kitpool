//! Group drawing shared by the RAM and Presets pages: dashed frames with a
//! name tab, the minimap strip, the jump row under a pane, the group menus
//! and where a slot drag lands. Everything here only reads; what the user
//! did comes back as [`GroupAct`]s (or a [`Landing`]) for the page to apply.

use super::*;
use crate::ui::slot_list::{DropRules, DropTarget, RowHit};
use crate::ui::theme::{self, oled_font, OLED_BG};
use egui::{Sense, Stroke};

/// Width of the strip between the slot rows and the scrollbar that holds the
/// group minimap.
pub const MINIMAP_W: f32 = 6.0;

/// Height of the group jump row under a pane's list.
pub const NAV_H: f32 = 28.0;

/// What the drawing needs to know about a page's groups.
pub struct GroupView<'a> {
    pub s: Slots<'a>,
    pub hues: &'a BTreeMap<String, f32>,
    /// The slot drag under way is this whole group's.
    pub drag: Option<usize>,
    /// A slot drag is under way.
    pub dragging: bool,
    pub grab: slot_list::Grab,
    /// Rows are this far apart on screen.
    pub pitch: f32,
}

impl GroupView<'_> {
    pub fn color(&self, name: &str, bright: bool) -> egui::Color32 {
        group_color(self.hues, name, bright)
    }
}

/// Dashed frames around the groups in `bank` (slots), with their name on a
/// tab. `y(s)`: the screen y of the boundary above slot `s` (hidden rows
/// take no height). The name tab ends at `tab_right`. The tab and the
/// frame's left and right edges grab the whole group; `drag` starts the
/// page's own drag payload.
#[allow(clippy::too_many_arguments)]
pub fn frames(
    ui: &mut egui::Ui,
    v: &GroupView,
    bank: Range<usize>,
    x: egui::Rangef,
    tab_right: f32,
    y: &dyn Fn(usize) -> f32,
    drag: &dyn Fn(&egui::Response),
    acts: &mut Vec<GroupAct>,
) {
    let clip = ui.clip_rect();
    let groups = v.s.groups;
    for (g, span) in v.s.group_spans().into_iter().enumerate() {
        if !bank.contains(&span.start) {
            continue;
        }
        let (top, bottom) = (y(span.start), y(span.end));
        if bottom - top < 1.0 {
            continue;
        }
        let rect = egui::Rect::from_x_y_ranges(x, top..=bottom).shrink2(egui::vec2(1.0, 0.5));
        if !clip.intersects(rect) {
            continue;
        }
        let name = &groups[g].name;
        let galley = ui.painter().layout_no_wrap(name.clone(), oled_font(15.0), OLED_BG);
        let tag_size = galley.size() + egui::vec2(14.0, 6.0);
        let tag_top = (rect.top() - tag_size.y / 2.0).max(clip.top()).min(rect.bottom() - tag_size.y);
        let tag = egui::Rect::from_min_size(egui::pos2(tab_right.min(rect.right()) - 12.0 - tag_size.x, tag_top), tag_size);
        let id = ui.scope_id().with(("group", g));
        let edge = 5.0;
        let tag_r = ui.interact(tag, id.with("tag"), Sense::click_and_drag());
        let left = ui.interact(
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + edge, rect.bottom())),
            id.with("left"),
            Sense::click_and_drag(),
        );
        let right = ui.interact(
            egui::Rect::from_min_max(egui::pos2(rect.right() - edge, rect.top()), rect.max),
            id.with("right"),
            Sense::click_and_drag(),
        );
        let dragging = v.drag == Some(g);
        // ⌥ held over a group drag: shade the half that a split would move.
        if dragging && ui.input(|i| i.modifiers.alt) && span.len() >= 2 {
            let half = span.len() / 2;
            let shade = egui::Rect::from_x_y_ranges(x, y(span.end - half)..=y(span.end));
            ui.painter().rect_filled(shade.intersect(clip), 0.0, v.color(name, false).gamma_multiply(0.35));
        }
        let hot = dragging || [&tag_r, &left, &right].iter().any(|r| r.hovered());
        if hot && !v.dragging {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let c = v.color(name, hot);
        let pts = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
        let p = ui.painter();
        let stroke = Stroke::new(if hot { 2.0 } else { 1.2 }, c);
        // Solid once all of the group is selected, dashed otherwise.
        if v.s.group_selected(g) {
            p.add(egui::Shape::line(pts.to_vec(), stroke));
        } else {
            p.extend(egui::Shape::dashed_line(&pts, stroke, 5.0, 3.0));
        }
        p.rect_filled(tag, 2.0, c);
        p.galley(tag.min + egui::vec2(5.0, 1.0), galley, OLED_BG);
        for r in [&tag_r, &left, &right] {
            if r.drag_started() {
                let py = ui.input(|i| i.pointer.press_origin()).map_or(rect.top(), |p| p.y);
                // The row grabbed: the first whose bottom is below the press.
                let s = span.clone().find(|&s| py < y(s + 1)).unwrap_or(span.end - 1);
                let frac = (py - y(s)) / v.pitch;
                acts.push(GroupAct::DragStart(g, s, frac));
                drag(r);
            }
        }
        if tag_r.clicked() {
            acts.push(GroupAct::Select(g));
        }
        for r in [&left, &right] {
            if r.clicked() {
                acts.push(GroupAct::Align(g));
            }
            r.clone().on_hover_text(format!(
                "Click: line the other pane up on its \"{name}\" group\nDrag: move the group"
            ));
        }
        if tag_r.double_clicked() {
            acts.push(GroupAct::Rename(g));
        }
        let tag_r = tag_r.on_hover_text(format!(
            "Group \"{name}\": {} slot(s), moves as one.\nDrag the name or the frame's side to move it · ⌥ while dropping splits it in two (second half, new group of the same name) · click the frame's side to line the other pane up on its \"{name}\" · drop slots on its rows to add them · drag one out to remove it\nDouble-click to rename · right-click for more",
            span.len()
        ));
        tag_r.context_menu(|ui| {
            menu(ui, v, g, acts);
            move_menus(ui, v, span.start / v.s.bank, &span.clone().collect::<Vec<_>>(), acts);
        });
    }
}

/// A 1px line per group in `bank`, beside the scrollbar, spanning the group's
/// slots scaled to the whole bank over the list's height.
pub fn minimap(ui: &egui::Ui, v: &GroupView, bank: Range<usize>, view: egui::Rect, bar: f32) {
    if bank.is_empty() {
        return;
    }
    let x = (view.right() - bar - MINIMAP_W / 2.0).round() + 0.5;
    let per = view.height() / bank.len() as f32;
    let painter = ui.painter().with_clip_rect(view);
    for (g, span) in v.s.group_spans().into_iter().enumerate() {
        if !bank.contains(&span.start) {
            continue;
        }
        let top = view.top() + (span.start - bank.start) as f32 * per;
        let bottom = (view.top() + (span.end.min(bank.end) - bank.start) as f32 * per).max(top + 2.0);
        painter.vline(x, top..=bottom, egui::Stroke::new(1.0, v.color(&v.s.groups[g].name, false)));
    }
}

/// The groups in `bank`, in slot order, for the jump row.
pub fn bank_groups(v: &GroupView, bank: usize) -> Vec<usize> {
    let spans = v.s.group_spans();
    let mut gs: Vec<usize> = (0..spans.len()).filter(|&g| spans[g].start / v.s.bank == bank).collect();
    gs.sort_by_key(|&g| spans[g].start);
    gs
}

/// One button per group in the bank a pane shows, under its list, in slot
/// order. Clicking one goes to the group and selects it. Slots dropped on a
/// button join that group (`plan_join_in_bank`). A group (dragged by its
/// button, name tab or frame) dropped between buttons moves to that place
/// among its bank's groups (`plan_reorder`); ⇧ keeps it to free room only
/// (`Shuffle::Fit`). `over(resp)`: the page's own drag hovers it.
pub fn nav_row(
    ui: &mut egui::Ui,
    v: &GroupView,
    salt: impl std::hash::Hash + std::fmt::Debug,
    bank_groups: &[usize],
    over: &dyn Fn(&egui::Response) -> bool,
    drag: &dyn Fn(&egui::Response),
) -> Vec<GroupAct> {
    let mut acts = Vec::new();
    let spans = v.s.group_spans();
    let (released, pointer, shift) =
        ui.input(|i| (i.pointer.any_released(), i.pointer.interact_pos(), i.modifiers.shift));
    let dragged = v.drag.filter(|&g| g < spans.len() && v.dragging);
    ui.allocate_ui(egui::vec2(ui.available_width(), NAV_H), |ui| {
        let row = ui.max_rect();
        ui.painter().rect_filled(row, 0.0, OLED_BG);
        let in_row = pointer.is_some_and(|p| row.contains(p));
        egui::ScrollArea::horizontal()
            .id_salt(("group_nav", salt))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let mut rects = Vec::new();
                    for &g in bank_groups {
                        let span = &spans[g];
                        let name = &v.s.groups[g].name;
                        let resp = ui
                            .selectable_label(v.s.group_selected(g) || dragged == Some(g), name.as_str())
                            .interact(Sense::click_and_drag());
                        rects.push((g, resp.rect));
                        let resp = resp.on_hover_text(format!(
                            "Click: go to group \"{name}\" ({}–{}, {} slot(s))\nDrag: move it between other groups' buttons to reorder, or onto the list\nDrop slots here: add them to the group",
                            v.s.label(span.start),
                            v.s.label(span.end - 1),
                            span.len()
                        ));
                        if resp.drag_started() {
                            acts.push(GroupAct::DragStart(g, span.start, 0.0));
                            drag(&resp);
                        }
                        if !v.dragging && resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                        }
                        let hovered = over(&resp) || (v.dragging && resp.contains_pointer());
                        if hovered && v.drag.is_none() {
                            let pick: Vec<usize> = v.s.sel.iter().copied().collect();
                            let (note, deny) = match v.s.plan_join_in_bank(g, &pick) {
                                Ok(p) => (p.note, false),
                                Err(why) => {
                                    let deny = !why.starts_with("Already");
                                    (why, deny)
                                }
                            };
                            let r = resp.rect.expand(2.0);
                            let stroke = if deny { theme::deny_frame() } else { Stroke::new(2.0, theme::DEVICE_ACCENT_BRIGHT) };
                            ui.painter().rect_stroke(r, 2.0, stroke, egui::StrokeKind::Outside);
                            theme::pointer_note(ui.ctx(), &note, if deny { theme::DENY } else { theme::DEVICE_ACCENT });
                            if (resp.contains_pointer() || over(&resp)) && released && v.dragging && !deny {
                                acts.push(GroupAct::Join(g, pick));
                            }
                        }
                        if resp.clicked() {
                            acts.push(GroupAct::GoTo(g));
                        }
                    }
                    // A group dragged over the row: the gap nearest the
                    // pointer among the bank's other groups is the target.
                    let (Some(dg), Some(p), true) = (dragged, pointer, in_row) else { return };
                    if !bank_groups.contains(&dg) {
                        theme::pointer_note(
                            ui.ctx(),
                            &format!("\"{}\" is in another bank: groups reorder within their own bank", v.s.groups[dg].name),
                            theme::DENY,
                        );
                        return;
                    }
                    let others: Vec<egui::Rect> = rects.iter().filter(|(g, _)| *g != dg).map(|(_, r)| *r).collect();
                    if others.is_empty() {
                        theme::pointer_note(ui.ctx(), "The only group in this bank: nothing to reorder against", theme::DENY);
                        return;
                    }
                    let at = others.iter().filter(|r| r.center().x < p.x).count();
                    let gap = ui.spacing().item_spacing.x / 2.0;
                    let x = match (at.checked_sub(1).map(|i| others[i]), others.get(at)) {
                        (Some(l), Some(r)) => (l.right() + r.left()) / 2.0,
                        (Some(l), None) => l.right() + gap,
                        (None, Some(r)) => r.left() - gap,
                        (None, None) => unreachable!(),
                    };
                    let cap = if shift { Shuffle::Fit } else { Shuffle::Rotate };
                    let (note, deny) = match v.s.plan_reorder(dg, at, cap) {
                        Ok(p) => {
                            let hint = if shift { "\nRelease ⇧ to push slots aside if needed" } else { "\n⇧ only into free slots" };
                            (p.note + hint, false)
                        }
                        Err(why) => {
                            let deny = !why.contains("already");
                            (why, deny)
                        }
                    };
                    let c = if deny { theme::DENY } else { theme::DEVICE_ACCENT_BRIGHT };
                    let y = others[0].y_range().expand(3.0);
                    ui.painter().vline(x, y, Stroke::new(3.0, c));
                    theme::pointer_note(ui.ctx(), &note, if deny { theme::DENY } else { theme::DEVICE_ACCENT });
                    if released && !deny {
                        acts.push(GroupAct::Reorder(dg, at, cap));
                    }
                });
            });
    });
    if !acts.is_empty() {
        // The buttons re-sort by slot order on the next frame; draw it now
        // rather than waiting for the pointer to move.
        ui.ctx().request_repaint();
    }
    acts
}

/// The group's right-click menu, on its name tab and inline in the slot
/// menu of its rows.
pub fn menu(ui: &mut egui::Ui, v: &GroupView, g: usize, acts: &mut Vec<GroupAct>) {
    if ui.button("Select group's slots").clicked() {
        acts.push(GroupAct::Select(g));
        ui.close();
    }
    if ui.button("Rename group…").clicked() {
        acts.push(GroupAct::Rename(g));
        ui.close();
    }
    match v.s.compactable(g) {
        Ok(n) => {
            if ui
                .button("Compact group")
                .on_hover_text(format!(
                    "Drop its {n} empty slot(s): the {} close up at the top, the freed slots stay just below, outside the group",
                    v.s.noun
                ))
                .clicked()
            {
                acts.push(GroupAct::Compact(g));
                ui.close();
            }
        }
        Err(why) => {
            ui.add_enabled(false, egui::Button::new("Compact group")).on_disabled_hover_text(why);
        }
    }
    spread_button(ui, v, g, acts);
    gather_button(ui, v, g, acts);
    if ui
        .button("Dissolve group")
        .on_hover_text(format!("Forget the group; its {} stay where they are", v.s.noun))
        .clicked()
    {
        acts.push(GroupAct::Dissolve(g));
        ui.close();
    }
    remove_empty_button(ui, v, acts);
    ui.separator();
    hue_slider(ui, v, g, acts);
}

/// "Remove N empty groups": every group on the page with nothing in it.
pub fn remove_empty_button(ui: &mut egui::Ui, v: &GroupView, acts: &mut Vec<GroupAct>) {
    let empty = v.s.empty_groups();
    if empty > 0 {
        let label = if empty == 1 { "Remove empty group".to_owned() } else { format!("Remove {empty} empty groups") };
        if ui
            .button(label)
            .on_hover_text(format!("Forget the groups that hold no {}. Slots don't move", v.s.noun))
            .clicked()
        {
            acts.push(GroupAct::RemoveEmpty);
            ui.close();
        }
    }
}

/// Colour of every group with this one's name: a hue offset from the device
/// yellow, -1..1 (±180°). Double-click the slider or Reset to clear it.
pub fn hue_slider(ui: &mut egui::Ui, v: &GroupView, g: usize, acts: &mut Vec<GroupAct>) {
    let Some(name) = v.s.groups.get(g).map(|gr| gr.name.clone()) else { return };
    let mut h = v.hues.get(&hue_key(&name)).copied().unwrap_or(0.0);
    let was = h;
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
        ui.painter().rect_filled(swatch, 2.0, v.color(&name, false));
        let resp = ui
            .add(egui::Slider::new(&mut h, -1.0..=1.0).text("Hue").fixed_decimals(2))
            .on_hover_text(format!("Colour of every group named \"{name}\", on every page, in all banks and projects\nDouble-click to reset"));
        if resp.double_clicked() {
            h = 0.0;
        }
        if was != 0.0 && ui.small_button("Reset").clicked() {
            h = 0.0;
        }
    });
    if h != was {
        acts.push(GroupAct::Hue(hue_key(&name), h));
    }
}

/// "Move toward the other banks": where same-named groups elsewhere keep
/// their slots (`Slots::plan_gather`), disabled with the reason if none.
fn gather_button(ui: &mut egui::Ui, v: &GroupView, g: usize, acts: &mut Vec<GroupAct>) {
    let Some(name) = v.s.groups.get(g).map(|gr| gr.name.clone()) else { return };
    let label = format!("Move toward \"{name}\" in other banks");
    match v.s.plan_gather(g) {
        Ok(plan) => {
            let tip = format!(
                "{}\nThe averaged slot offsets of the other banks' {}; slots in this bank move out of the way",
                plan.note, v.s.noun
            );
            if ui.button(label).on_hover_text(tip).clicked() {
                acts.push(GroupAct::Gather(g));
                ui.close();
            }
        }
        Err(why) => {
            ui.add_enabled(false, egui::Button::new(label)).on_disabled_hover_text(why);
        }
    }
}

/// "Add empty "fx" to other banks": an empty container like this group in
/// every bank that has none, on the same slot numbers when they're free.
fn spread_button(ui: &mut egui::Ui, v: &GroupView, g: usize, acts: &mut Vec<GroupAct>) {
    let Some(name) = v.s.groups.get(g).map(|gr| gr.name.clone()) else { return };
    let targets = v.s.spread_targets(g);
    let label = format!("Add empty \"{name}\" to other banks");
    let names = |f: &dyn Fn(Option<usize>) -> bool| {
        targets.iter().filter(|(_, at)| f(*at)).map(|(b, _)| v.s.bname(*b)).collect::<Vec<_>>().join(", ")
    };
    let fits = names(&|at| at.is_some());
    let full = names(&|at| at.is_none());
    if fits.is_empty() {
        let why = if targets.is_empty() {
            format!("Every bank already has a \"{name}\" group")
        } else {
            format!("No bank without \"{name}\" has an empty slot ({full})")
        };
        ui.add_enabled(false, egui::Button::new(label)).on_disabled_hover_text(why);
        return;
    }
    let mut tip = format!(
        "Make a one-slot empty \"{name}\" group in {fits}, on the slot number in the middle of this group where it's empty, else the nearest empty slot.\nDrop {} on it to fill it.",
        v.s.noun
    );
    if !full.is_empty() {
        tip += &format!("\nNo room in {full}");
    }
    if ui.button(label).on_hover_text(tip).clicked() {
        acts.push(GroupAct::Spread(g));
        ui.close();
    }
}

/// The group part of a slot's right-click menu: its group's menu, or
/// "Group N slots…" for the selection (or this row); then "Move to group"
/// in this bank and in the others. `selected`: the row is in the selection.
pub fn slot_menu(ui: &mut egui::Ui, v: &GroupView, slot: usize, selected: bool, acts: &mut Vec<GroupAct>) {
    let s = &v.s;
    let pick: Vec<usize> = if selected { s.sel.iter().copied().collect() } else { vec![slot] };
    if let Some(g) = s.group_at(slot) {
        menu(ui, v, g, acts);
    } else {
        let label = if pick.len() == 1 { "Make a group of this slot…".to_owned() } else { format!("Group {} slots…", pick.len()) };
        match s.groupable(&pick) {
            Ok(()) => {
                if ui
                    .button(label)
                    .on_hover_text("Name it; the slots then move as one. Drop slots on it to add them, drag them out to remove them")
                    .clicked()
                {
                    acts.push(GroupAct::New(pick.clone()));
                    ui.close();
                }
            }
            Err(why) => {
                ui.add_enabled(false, egui::Button::new(label)).on_disabled_hover_text(why);
            }
        }
    }
    move_menus(ui, v, slot / s.bank, &pick, acts);
    if s.group_at(slot).is_none() {
        remove_empty_button(ui, v, acts);
    }
}

/// "Move to group in this bank" / "…in other bank" for slots `pick`, with
/// `here` the bank they're clicked in. Shown on a slot's right-click menu
/// (the selection or the row) and on a group's name tab (all its slots).
fn move_menus(ui: &mut egui::Ui, v: &GroupView, here: usize, pick: &[usize], acts: &mut Vec<GroupAct>) {
    let s = &v.s;
    // Move the selection (or this row) into one of the groups. Each entry
    // says what would happen; a move that would split or overflow a group
    // is shown disabled, with the reason. Split by bank: groups in the
    // clicked slot's bank, and every group in the other banks (headed by
    // bank name, in slot order).
    if !s.groups.is_empty() {
        let spans = s.group_spans();
        let mut by_slot: Vec<usize> = (0..s.groups.len()).collect();
        by_slot.sort_by_key(|&g| spans[g].start);
        let (mine, other): (Vec<usize>, Vec<usize>) = by_slot.into_iter().partition(|&g| spans[g].start / s.bank == here);
        let entry = |ui: &mut egui::Ui, acts: &mut Vec<GroupAct>, g: usize| {
            let name = s.groups[g].name.as_str();
            match s.plan_join_in_bank(g, pick) {
                Ok(plan) => {
                    if ui.button(name).on_hover_text(plan.note).clicked() {
                        acts.push(GroupAct::Join(g, pick.to_vec()));
                        ui.close();
                    }
                }
                Err(why) => {
                    ui.add_enabled(false, egui::Button::new(name)).on_disabled_hover_text(why);
                }
            }
        };
        ui.separator();
        if mine.is_empty() {
            ui.add_enabled(false, egui::Button::new("Move to group in this bank"))
                .on_disabled_hover_text(format!("Bank {} has no groups", s.bname(here)));
        } else {
            ui.menu_button("Move to group in this bank", |ui| {
                for &g in &mine {
                    entry(ui, acts, g);
                }
            });
        }
        if other.is_empty() {
            ui.add_enabled(false, egui::Button::new("Move to group in other bank"))
                .on_disabled_hover_text("No other bank has groups");
        } else {
            ui.menu_button("Move to group in other bank", |ui| {
                let mut last = None;
                for &g in &other {
                    let b = spans[g].start / s.bank;
                    if last != Some(b) {
                        if last.is_some() {
                            ui.separator();
                        }
                        ui.label(egui::RichText::new(format!("Bank {}", s.bname(b))).weak().small());
                        last = Some(b);
                    }
                    entry(ui, acts, g);
                }
            });
        }
    }
}

/// Where a slot drag over a row lands, before it happens.
pub struct Landing {
    pub drop: SlotDrop,
    /// For the feedback: the block's top (or the line, joining a group).
    pub target: DropTarget,
    /// Rows the landing band covers.
    pub n: usize,
    pub note: String,
    pub deny: bool,
    /// Into a group: tint the area, no insertion line.
    pub into_group: bool,
    /// The order it makes (`order[slot] = id`), when allowed.
    pub order: Option<Vec<u16>>,
}

/// Where a slot drag over `hit` lands, its feedback and its note. A plain
/// drop pushes what's there down to the next empty slot (or inserts, on a
/// page that inserts); over a group's rows it joins the group; ⌥ swaps
/// (groups stay out of swaps). A group dragged by its name or frame lands
/// whole, never inside another group. `rules.plain` must be `Shift` (the
/// targets are block tops).
pub fn landing(ui: &egui::Ui, v: &GroupView, hit: &RowHit, rules: &DropRules) -> Landing {
    let s = &v.s;
    let spans = s.group_spans();
    let slot = (hit.slot_of)(hit.row);
    let (alt, opt, py) =
        ui.input(|i| (i.modifiers.alt || i.modifiers.ctrl, i.modifiers.alt, i.pointer.interact_pos().map(|p| p.y)));
    let frac = py.map_or(0.5, |y| (y - hit.rect.top()) / hit.pitch);
    let top_of = |t: DropTarget| match t {
        DropTarget::Place(t) | DropTarget::Insert(t) | DropTarget::Shift(t) => t,
    };
    let held_top = |n: usize| top_of(slot_list::drop_target_rows(ui, hit, n, v.grab, rules));
    // A block's top strictly inside a group: just past it.
    let clear_of = |top: usize, skip: Option<usize>| {
        spans
            .iter()
            .enumerate()
            .find(|(i, r)| Some(*i) != skip && r.start < top && top < r.end)
            .map_or(top, |(_, r)| if skip.is_some() && top - r.start <= r.end - top { r.start } else { r.end })
    };
    let n_sel = s.sel.len().max(1);
    let (d, n) = if let Some(g) = v.drag.filter(|&g| g < spans.len()) {
        let len = spans[g].len();
        if opt {
            // ⌥ splits: the landing band is the half that moves. The moved
            // half starts `len - half` rows into the group, so the grab
            // offset is measured from its top, not the group's.
            let half = (len / 2).max(1);
            let mut grab = v.grab;
            grab.before = grab.before.saturating_sub(len - half);
            let top = top_of(slot_list::drop_target_rows(ui, hit, half, grab, rules));
            (SlotDrop::Split { g, top: clear_of(top, None) }, half)
        } else {
            (SlotDrop::Group { g, top: clear_of(held_top(len), Some(g)) }, len)
        }
    } else if alt {
        (SlotDrop::Place(held_top(n_sel)), n_sel)
    } else if let Some(g) = spans.iter().position(|r| r.contains(&slot)) {
        let line = (slot + usize::from(frac > 0.5)).clamp(spans[g].start, spans[g].end);
        let pos = positions(s.order);
        let moving: HashSet<u16> = s.sel.iter().filter_map(|&x| s.order.get(x).copied()).collect();
        let before = s.groups[g]
            .members
            .iter()
            .copied()
            .find(|&x| pos[x as usize] >= line && !moving.contains(&x));
        (SlotDrop::Into { g, before, line }, n_sel)
    } else {
        (SlotDrop::Shift(clear_of(held_top(n_sel), None)), n_sel)
    };
    let plain = if s.insert { "insert" } else { "push down" };
    let (target, hint) = match d {
        SlotDrop::Into { line, .. } => (DropTarget::Insert(line), "\n⌥ swap in place instead".to_owned()),
        SlotDrop::Place(t) => (DropTarget::Place(t), format!("\nRelease ⌥ to {plain} instead")),
        SlotDrop::Shift(t) => (DropTarget::Shift(t), "\n⌥ swap in place instead".to_owned()),
        SlotDrop::Group { top, .. } => (DropTarget::Shift(top), "\n⌥ split it in two instead".to_owned()),
        SlotDrop::Split { top, .. } => (DropTarget::Shift(top), "\nRelease ⌥ to move the whole group".to_owned()),
    };
    let (note, deny, order) = match s.plan(d) {
        Ok(p) => (p.note + &hint, false, Some(p.order)),
        Err(why) => {
            let deny = !why.starts_with("Already");
            (why, deny, None)
        }
    };
    Landing { drop: d, target, n, note, deny, into_group: matches!(d, SlotDrop::Into { .. }), order }
}
