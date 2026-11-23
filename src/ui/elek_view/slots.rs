//! Sample-pool bank panes, slot drawing and drag/drop action collection.

use super::*;

/// Background behind selected rows that aren't the lone inverted highlight.
const SEL_GREY: Color32 = Color32::from_rgb(62, 72, 70);

/// Shared facts for a sample-slot row's drop feedback, menu and hover text.
pub(super) struct SlotRowInfo<'a> {
    pub(super) slot: usize,
    pub(super) from: usize,
    pub(super) entry: PoolEntry,
    pub(super) linked: Option<&'a (String, std::path::PathBuf, bool)>,
    pub(super) changed: bool,
    pub(super) selected: bool,
    pub(super) moved: bool,
    pub(super) uses: u32,
    pub(super) rect: egui::Rect,
}

pub(super) fn body(ui: &mut egui::Ui, st: &mut ElekState, out: &mut Vec<ElekIntent>, busy: bool) {
    if st.project.as_ref().is_some_and(|p| p.partial) && busy {
        ui.label(RichText::new("Sample pool loaded; use counts fill in as the patterns arrive. You can start arranging, pushing unlocks when the read finishes.").weak());
    } else if st.project.as_ref().is_some_and(|p| p.partial) {
        ui.colored_label(
            Color32::from_rgb(230, 120, 90),
            "The read was stopped: use counts are incomplete and sample order changes can't be pushed. Reload to read the whole project.",
        );
    } else if let Some(why) = st.project.as_ref().and_then(|p| p.read_only.clone()) {
        ui.colored_label(Color32::from_rgb(230, 120, 90), format!("Read-only: {why}"))
            .on_hover_text(
                "Pushing is disabled because the dump doesn't match the layouts we know.",
            );
    }

    if st.project.is_none() {
        load_screen(ui, st, out);
    } else {
        st.keys(ui.ctx());

        let w = (ui.available_width() - LINK_GAP) / 2.0;
        let h = (ui.available_height() - MEM_BAR_H).max(ROW_H * 4.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for side in 0..2 {
                if side == 1 {
                    scroll_link_toggle(ui, st);
                }
                ui.allocate_ui_with_layout(
                    egui::vec2(w, h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_size(egui::vec2(w, h));
                        ui.push_id(side, |ui| slot_pane(ui, st, side));
                    },
                );
            }
        });
        memory_bar(ui, st);
    }
}

/// Width of the column between the two panes that holds the scroll padlock.
const LINK_GAP: f32 = 18.0;

/// The padlock at the top between the panes: locked, both scroll together.
fn scroll_link_toggle(ui: &mut egui::Ui, st: &mut ElekState) {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(LINK_GAP, ROW_H + 6.0), Sense::click());
    let resp = resp.on_hover_text(if st.scroll_linked {
        "Panes scroll together · click to unlink"
    } else {
        "Link the two panes' scrolling: move one, the other follows"
    });
    if resp.clicked() {
        st.scroll_linked = !st.scroll_linked;
        if st.scroll_linked {
            let from = st.active.min(1);
            st.follow[1 - from] = Some(st.scroll[from]);
            ui.ctx().request_repaint();
        }
    }
    let hot = resp.hovered();
    let col = match (st.scroll_linked, hot) {
        (true, _) => theme::DEVICE_ACCENT_BRIGHT,
        (false, true) => OLED,
        (false, false) => OLED_DIM,
    };
    let p = ui.painter();
    let c = r.center();
    let body = egui::Rect::from_center_size(egui::pos2(c.x, c.y + 3.0), egui::vec2(10.0, 7.0));
    p.rect_filled(body, 1.5, col);
    // Shackle: an upside-down U; unlocked, its left leg lifts out.
    let (l, rr, top) = (body.left() + 2.0, body.right() - 2.0, body.top() - 5.5);
    let lift = if st.scroll_linked { 0.0 } else { 3.0 };
    let stroke = Stroke::new(1.6, col);
    let pts: Vec<egui::Pos2> = (0..=8)
        .map(|k| {
            let t = std::f32::consts::PI * k as f32 / 8.0;
            egui::pos2(c.x - (rr - l) / 2.0 * t.cos(), top - lift - 2.5 * t.sin() + 2.5)
        })
        .collect();
    p.add(egui::Shape::line(pts, stroke));
    p.line_segment([egui::pos2(rr, top + 2.5 - lift), egui::pos2(rr, body.top())], stroke);
    p.line_segment([egui::pos2(l, top + 2.5 - lift), egui::pos2(l, body.top() - lift)], stroke);
}

pub(super) fn slot_pane(ui: &mut egui::Ui, st: &mut ElekState, side: usize) {
    // A slot drag ends with the button release (handled the frame it
    // happens) or Escape.
    if ui.input(|i| (!i.pointer.any_down() && !i.pointer.any_released()) || i.key_pressed(egui::Key::Escape)) {
        st.slot_drag = false;
        st.group_drag = None;
    }
    let Some(p) = &st.project else { return };
    let n = p.pool.len();
    let banks = n.div_ceil(BANK);
    // Keep the keyboard cursor visible in the pane it drives, and a pinged
    // slot in the pane picked for it.
    let target = st
        .reveal
        .filter(|_| side == st.active)
        .or_else(|| st.ping.filter(|_| st.ping_scroll == Some(side)).map(|(pos, _)| pos));
    if let Some(r) = target {
        st.bank[side] = r / BANK;
    }
    if banks > 1 {
        // Device-style bank strip: "< A B C D E F G H >", current one boxed.
        let (strip, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H + 6.0),
            Sense::empty(),
        );
        ui.painter().rect_filled(strip, 0.0, OLED_BG);
        // Narrow cells in a narrow pane so the strip never spills past the
        // left edge (it is centred, so overflow would be clipped there).
        let cells = banks as f32 + 2.0;
        let cell = (strip.width() / cells).clamp(12.0, 26.0);
        let x0 = (strip.center().x - cell * cells / 2.0).max(strip.left());
        let font = oled_font(TEXT);
        let arrow = |ui: &mut egui::Ui, i: f32, txt: &str, to: Option<usize>| {
            let r = egui::Rect::from_min_size(
                egui::pos2(x0 + i * cell, strip.top() + 3.0),
                egui::vec2(cell, ROW_H),
            );
            let resp = ui.interact(r, ui.scope_id().with(("bank_tab", txt)), Sense::click());
            let on = to.is_some();
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                txt,
                font.clone(),
                if on && resp.hovered() { OLED } else { OLED_DIM },
            );
            (resp, to)
        };
        let mut pick = None;
        let cur = st.bank[side];
        let (r, to) = arrow(ui, 0.0, "<", cur.checked_sub(1));
        if r.clicked() {
            pick = to;
        }
        for b in 0..banks {
            let r = egui::Rect::from_min_size(
                egui::pos2(x0 + (b + 1) as f32 * cell, strip.top() + 3.0),
                egui::vec2(cell, ROW_H),
            );
            let resp = ui.interact(r, ui.scope_id().with(("bank", b)), Sense::click());
            let letter = ((b'A' + b as u8) as char).to_string();
            let p = ui.painter();
            if b == cur {
                p.rect_stroke(
                    r.shrink2(egui::vec2(4.0, 0.0)),
                    0.0,
                    Stroke::new(2.0, OLED),
                    egui::StrokeKind::Inside,
                );
            } else if resp.hovered() {
                p.rect_filled(
                    r.shrink2(egui::vec2(4.0, 0.0)),
                    0.0,
                    OLED_HOVER,
                );
            }
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                letter,
                font.clone(),
                OLED,
            );
            theme::color_badge(p, r.shrink2(egui::vec2(4.0, 0.0)), theme::BadgeEdge::Bottom, theme::identity_color(b, banks));
            // Dragging slots over a bank tab opens that bank.
            if resp.clicked() || resp.dnd_hover_payload::<SlotDrag>().is_some() {
                pick = Some(b);
            }
        }
        let (r, to) = arrow(
            ui,
            banks as f32 + 1.0,
            ">",
            Some(cur + 1).filter(|&b| b < banks),
        );
        if r.clicked() {
            pick = to;
        }
        if let Some(b) = pick {
            st.bank[side] = b;
            st.active = side;
        }
    }
    let bank = st.bank[side].min(banks - 1);
    let first = bank * BANK;
    let rows = BANK.min(n - first);
    // Jump row under the list: only when this bank has groups.
    let spans = st.group_spans();
    let mut bank_groups: Vec<usize> = (0..spans.len()).filter(|&g| spans[g].start / BANK == bank).collect();
    bank_groups.sort_by_key(|&g| spans[g].start);
    let nav_h = if bank_groups.is_empty() { 0.0 } else { GROUP_NAV_H };
    let mut acts = Vec::new();
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("elek_slots", side, bank))
        .auto_shrink([false, false]);
    let motion_id = ui.scope_id().with(("pool_ping_scroll", side, bank));
    let mut ping_target = None;
    if let Some(r) = target.filter(|&r| r >= first) {
        let y = (r - first) as f32 * ROW_H;
        let (top, h) = (st.scroll[side], st.view_h[side]);
        let destination = if y < top { Some(y) } else if y + ROW_H > top + h {
            Some(y + ROW_H - h)
        } else { None };
        if st.reveal.is_some() {
            ui.data_mut(|d| d.remove::<(f64, f32, f32)>(motion_id));
            if let Some(y) = destination {
                scroll = scroll.vertical_scroll_offset(y);
            }
        } else {
            ping_target = destination.map(|y| y.clamp(0.0, (rows as f32 * ROW_H - h).max(0.0)));
        }
    }
    if let Some(y) = st.align[side].take() {
        ping_target = Some(y);
    }
    if let Some(y) = crate::ui::file_table::ping_scroll(ui, motion_id, st.scroll[side], ping_target) {
        scroll = scroll.vertical_scroll_offset(y);
    }
    // Linked to the other pane, which moved: jump to its offset. A move
    // caused this way isn't passed back (a shorter bank clamps it, and
    // passing that back would drag the other pane up).
    let followed = match st.follow[side].take() {
        Some(y) if st.scroll_linked => {
            ui.data_mut(|d| d.remove::<(f64, f32, f32)>(motion_id));
            scroll = scroll.vertical_scroll_offset(y);
            true
        }
        _ => false,
    };
    let list_h = (ui.available_height() - nav_h).max(ROW_H * 2.0);
    let out = ui.allocate_ui(egui::vec2(ui.available_width(), list_h), |ui| {
        egui::Frame::new()
        .fill(OLED_BG)
        .corner_radius(2.0)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            // show_rows sizes the list with the spacing set *before* it is
            // called, so zero it here, not inside, or the list gets a blank
            // tail of rows × spacing.
            ui.spacing_mut().item_spacing.y = 0.0;
            theme::always_show_scrollbar(ui);
            // Room on the right for the group minimap, clear of the
            // (floating) scrollbar.
            let bar = {
                let sc = &ui.spacing().scroll;
                if sc.floating { sc.bar_width + sc.bar_outer_margin } else { 0.0 }
            };
            let shown = scroll.show_rows(ui, ROW_H, rows, |ui, range| {
                ui.set_max_width((ui.available_width() - bar - MINIMAP_W).max(0.0));
                let mut landing = None;
                let mut top_row = None;
                for i in range {
                    let r = slot_row(ui, st, first + i, &mut acts, &mut landing);
                    top_row.get_or_insert((first + i, r));
                }
                if let Some(top_row) = top_row {
                    group_frames(ui, st, first..first + rows, top_row, &mut acts);
                }
                // Painted over all rows, so later rows' fills don't hide it.
                if let Some((x, y, target, note, deny, into_group)) = landing {
                    if into_group {
                        let c = if deny { theme::DENY } else { theme::DEVICE_ACCENT_BRIGHT };
                        ui.painter().rect_filled(egui::Rect::from_x_y_ranges(x, y), 0.0, c.gamma_multiply(0.10));
                    } else {
                        slot_list::paint_landing(ui, x, y, target, deny);
                    }
                    theme::pointer_note(ui.ctx(), &note, if deny { theme::DENY } else { theme::DEVICE_ACCENT });
                }
                if st.slot_drag {
                    slot_list::drag_autoscroll(ui, ROW_H * 1.5);
                }
            });
            group_minimap(ui, st, first..first + rows, shown.inner_rect, bar);
            shown
        })
    }).inner;
    let y = out.inner.state.clamped_offset().y;
    if st.scroll_linked && !st.docked && !followed && (y - st.scroll[side]).abs() > 0.5 {
        st.follow[1 - side] = Some(y);
        ui.ctx().request_repaint();
    }
    st.scroll[side] = y;
    st.view_h[side] = out.inner.inner_rect.height();
    if nav_h > 0.0 {
        group_nav_row(ui, st, side, &bank_groups, &spans);
    }
    if side == st.active {
        st.reveal = None;
    }
    if st.ping_scroll == Some(side) {
        st.ping_scroll = None;
    }
    if st.ping.is_some_and(|(_, at)| at.elapsed().as_secs_f32() >= crate::ui::file_table::PING_SECS) {
        st.ping = None;
    }
    let locked = st.edit_locked();
    for a in acts {
        let edits = !matches!(
            a,
            RowAct::Click(..) | RowAct::DragStart(..) | RowAct::GroupDragStart(..) | RowAct::NewGroup(_) | RowAct::RenameGroup(_) | RowAct::SelectGroup(_) | RowAct::AlignGroup(_) | RowAct::Local(..) | RowAct::ShowOriginal(_) | RowAct::Rename(_) | RowAct::Fetch(_) | RowAct::Clip(..) | RowAct::DropClip
        );
        if locked && edits {
            st.status = "Wait a moment: the device is busy, and an edit now would be lost".into();
            continue;
        }
        match a {
            RowAct::Click(s, m) => {
                st.active = side;
                // A plain click inside a group first selects the whole group;
                // the next plain click picks one of its rows.
                let plain = !m.shift && !m.command;
                let group = st.group_at(s).filter(|&g| plain && !st.group_selected(g));
                if let Some(g) = group {
                    st.select_group(g);
                } else {
                    st.click(s, m);
                }
                if group.is_none() && (st.preview || st.open_on_click) && plain {
                    if let Some(k) = st.key_at(s) {
                        st.want_local = Some((k, st.open_on_click));
                    }
                }
            }
            RowAct::DragStart(s, frac) => {
                st.slot_drag = true;
                st.group_drag = None;
                st.active = side;
                if !st.sel.contains(&s) {
                    st.click(s, egui::Modifiers::NONE);
                }
                st.slot_grab = slot_list::Grab::new(&st.sel, s, frac);
            }
            RowAct::GroupDragStart(g, s, frac) => {
                st.slot_drag = true;
                st.group_drag = Some(g);
                st.active = side;
                st.select_group(g);
                st.slot_grab = slot_list::Grab::new(&st.sel, s, frac);
            }
            RowAct::Drop(d) => {
                st.active = side;
                st.apply_drop(d);
            }
            RowAct::NewGroup(slots) => st.group_naming = Some((None, slots, String::new(), true)),
            RowAct::RenameGroup(g) => {
                if let Some(gr) = st.groups.get(g) {
                    st.group_naming = Some((Some(g), Vec::new(), gr.name.clone(), true));
                }
            }
            RowAct::DissolveGroup(g) => st.dissolve_group(g),
            RowAct::CompactGroup(g) => st.compact_group(g),
            RowAct::GroupHue(name, h) => {
                if h == 0.0 {
                    st.group_hues.remove(&name);
                } else {
                    st.group_hues.insert(name, h);
                }
                st.hues_dirty = true;
            }
            RowAct::SelectGroup(g) => {
                st.active = side;
                st.select_group(g);
            }
            RowAct::AlignGroup(g) => st.align_group(g, side),
            RowAct::SpreadGroup(g) => st.spread_group(g),
            RowAct::GatherGroup(g) => match st.plan_gather(g) {
                Ok(p) => {
                    st.commit(p.order, p.groups);
                    st.status = p.note;
                }
                Err(why) => st.status = why,
            },
            RowAct::RemoveEmptyGroups => st.remove_empty_groups(),
            RowAct::JoinGroup(g, pick) => {
                st.active = side;
                st.join_group(g, &pick);
            }
            RowAct::InsertEmpty(s, n) => st.insert_empty(s, n),
            RowAct::CloseGap(s) => st.close_gap(s),
            RowAct::MoveHere(s) => st.move_sel_to(s),
            RowAct::Nudge(d) => st.nudge(d),
            RowAct::Clear(slots) => st.clear(&slots),
            RowAct::ClearUnused => {
                let slots = st.unused_slots();
                st.clear(&slots);
            }
            RowAct::Local(k, open) => st.want_local = Some((k, open)),
            RowAct::ShowOriginal(k) => st.want_original = Some(k),
            RowAct::Fetch(keys) => st.want_fetch = Some(keys),
            RowAct::UpdateChanged(keys) => st.want_update = Some(keys),
            RowAct::Rename(k) => {
                if let Some(path) = st.names.get(&k).cloned() {
                    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                    st.renaming = Some((k, path, name, true));
                }
            }
            RowAct::DropFiles(s, files) => {
                st.active = side;
                st.drop_files(s, files);
            }
            RowAct::Clip(slots, cut) => st.set_clip(&slots, cut, ui.ctx()),
            RowAct::Paste(s, shift) => {
                st.active = side;
                st.paste(s, shift);
            }
            RowAct::DropClip => {
                st.clip = None;
                st.status = "Cut cancelled".into();
            }
        }
    }
}

/// Width of the strip between the slot rows and the scrollbar that holds the
/// group minimap.
const MINIMAP_W: f32 = 6.0;

/// A 1px line per group in `bank`, beside the scrollbar, spanning the group's
/// slots scaled to the whole bank over the list's height.
fn group_minimap(ui: &egui::Ui, st: &ElekState, bank: std::ops::Range<usize>, view: egui::Rect, bar: f32) {
    if bank.is_empty() {
        return;
    }
    let x = (view.right() - bar - MINIMAP_W / 2.0).round() + 0.5;
    let per = view.height() / bank.len() as f32;
    let painter = ui.painter().with_clip_rect(view);
    for (g, span) in st.group_spans().into_iter().enumerate() {
        if !bank.contains(&span.start) {
            continue;
        }
        let top = view.top() + (span.start - bank.start) as f32 * per;
        let bottom = (view.top() + (span.end.min(bank.end) - bank.start) as f32 * per).max(top + 2.0);
        let c = st.group_color(&st.groups[g].name, false);
        painter.vline(x, top..=bottom, egui::Stroke::new(1.0, c));
    }
}

/// Height of the group jump row under a pane's list.
const GROUP_NAV_H: f32 = ROW_H + 8.0;

/// One button per group in the bank pane `side` shows, under its list, in
/// slot order. Clicking one scrolls the pane to the group and selects it.
/// Slots dropped on a button join that group (`plan_join_in_bank`). A group
/// (dragged by its button, name tab or frame) dropped between buttons moves
/// to that place among its bank's groups (`plan_reorder`); ⇧ keeps it to
/// free room only (`Shuffle::Fit`).
fn group_nav_row(ui: &mut egui::Ui, st: &mut ElekState, side: usize, bank_groups: &[usize], spans: &[std::ops::Range<usize>]) {
    use groups::Shuffle;
    let mut go = None;
    let mut join = None;
    let mut reorder = None;
    let mut drag_from = None;
    let (released, pointer, shift) =
        ui.input(|i| (i.pointer.any_released(), i.pointer.interact_pos(), i.modifiers.shift));
    let dragged = st.group_drag.filter(|&g| g < spans.len() && st.slot_drag);
    ui.allocate_ui(egui::vec2(ui.available_width(), GROUP_NAV_H), |ui| {
        let row = ui.max_rect();
        ui.painter().rect_filled(row, 0.0, OLED_BG);
        let in_row = pointer.is_some_and(|p| row.contains(p));
        egui::ScrollArea::horizontal()
            .id_salt(("group_nav", side))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let mut rects = Vec::new();
                    for &g in bank_groups {
                        let span = &spans[g];
                        let name = &st.groups[g].name;
                        let resp = ui
                            .selectable_label(st.group_selected(g) || dragged == Some(g), name.as_str())
                            .interact(Sense::click_and_drag());
                        rects.push((g, resp.rect));
                        let resp = resp.on_hover_text(format!(
                            "Click: go to group \"{name}\" ({}–{}, {} slot(s))\nDrag: move it between other groups' buttons to reorder, or onto the list\nDrop slots here: add them to the group",
                            st.label(span.start),
                            st.label(span.end - 1),
                            span.len()
                        ));
                        if resp.drag_started() {
                            drag_from = Some((g, span.start));
                            resp.dnd_set_drag_payload(SlotDrag);
                        }
                        if !st.slot_drag && resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                        }
                        let over = resp.dnd_hover_payload::<SlotDrag>().is_some()
                            || (st.slot_drag && resp.contains_pointer());
                        if over && st.group_drag.is_none() {
                            let pick: Vec<usize> = st.sel.iter().copied().collect();
                            let (note, deny) = match st.plan_join_in_bank(g, &pick) {
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
                            if (resp.contains_pointer() || resp.dnd_hover_payload::<SlotDrag>().is_some())
                                && released
                                && st.slot_drag
                                && !deny
                            {
                                join = Some((g, pick));
                            }
                        }
                        if resp.clicked() {
                            go = Some(g);
                        }
                    }
                    // A group dragged over the row: the gap nearest the
                    // pointer among the bank's other groups is the target.
                    let (Some(dg), Some(p), true) = (dragged, pointer, in_row) else { return };
                    if !bank_groups.contains(&dg) {
                        theme::pointer_note(
                            ui.ctx(),
                            &format!("\"{}\" is in another bank: groups reorder within their own bank", st.groups[dg].name),
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
                    let (note, deny) = match st.plan_reorder(dg, at, cap) {
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
                        reorder = Some((dg, at, cap));
                    }
                });
            });
    });
    if let Some((g, top)) = drag_from {
        st.slot_drag = true;
        st.group_drag = Some(g);
        st.active = side;
        st.select_group(g);
        st.slot_grab = slot_list::Grab::new(&st.sel, top, 0.0);
    }
    if let Some((g, at, cap)) = reorder {
        st.active = side;
        match st.plan_reorder(g, at, cap) {
            Ok(p) => {
                st.commit(p.order, p.groups);
                st.select_group(g);
                st.status = p.note;
            }
            Err(why) => st.status = why,
        }
        st.slot_drag = false;
        st.group_drag = None;
        // The buttons re-sort by slot order on the next frame; draw it now
        // rather than waiting for the pointer to move.
        ui.ctx().request_repaint();
    } else if let Some((g, pick)) = join {
        st.active = side;
        st.join_group(g, &pick);
        ui.ctx().request_repaint();
    } else if let Some(g) = go {
        st.go_to_group(g, side);
        ui.ctx().request_repaint();
    }
}

/// Drop feedback to paint once the rows are drawn: `(row x range, landing
/// y range, target, pointer note, refused, into a group)`, and the drop itself.
/// Into a group the area is tinted without the insertion line.
type Landing = (egui::Rangef, egui::Rangef, DropTarget, String, bool, bool);

/// Where a slot drag over `slot` lands, its feedback and its note. A plain
/// drop pushes what's there down to the next empty slot; over a group's
/// rows it joins the group; ⌥ swaps (groups stay out of swaps). A group
/// dragged by its name or frame lands whole, never inside another group.
fn slot_landing(ui: &egui::Ui, st: &ElekState, rect: egui::Rect, slot: usize) -> (Landing, groups::SlotDrop) {
    use groups::SlotDrop;
    let rules = slot_list::DropRules { first: 1, len: st.order.len(), plain: slot_list::PlainDrop::Shift };
    let spans = st.group_spans();
    let (alt, opt, py) =
        ui.input(|i| (i.modifiers.alt || i.modifiers.ctrl, i.modifiers.alt, i.pointer.interact_pos().map(|p| p.y)));
    let frac = py.map_or(0.5, |y| (y - rect.top()) / ROW_H);
    let hit = slot_list::RowHit { rect, pitch: ROW_H, row: slot, slot_of: &|r| r };
    let held_top = |n: usize| match slot_list::drop_target_rows(ui, &hit, n, st.slot_grab, &rules) {
        DropTarget::Place(t) | DropTarget::Insert(t) | DropTarget::Shift(t) => t,
    };
    // A block's top strictly inside a group: just past it.
    let clear_of = |top: usize, skip: Option<usize>| {
        spans
            .iter()
            .enumerate()
            .find(|(i, r)| Some(*i) != skip && r.start < top && top < r.end)
            .map_or(top, |(_, r)| if skip.is_some() && top - r.start <= r.end - top { r.start } else { r.end })
    };
    let n_sel = st.sel.len().max(1);
    let (d, n) = if let Some(g) = st.group_drag.filter(|&g| g < spans.len()) {
        let len = spans[g].len();
        if opt {
            // ⌥ splits: the landing band is the half that moves.
            // The moved half starts `len - half` rows into the group, so the
            // grab offset is measured from its top, not the group's.
            let half = (len / 2).max(1);
            let mut grab = st.slot_grab;
            grab.before = grab.before.saturating_sub(len - half);
            let top = match slot_list::drop_target_rows(ui, &hit, half, grab, &rules) {
                DropTarget::Place(t) | DropTarget::Insert(t) | DropTarget::Shift(t) => t,
            };
            (SlotDrop::Split { g, top: clear_of(top, None) }, half)
        } else {
            (SlotDrop::Group { g, top: clear_of(held_top(len), Some(g)) }, len)
        }
    } else if alt {
        (SlotDrop::Place(held_top(n_sel)), n_sel)
    } else if let Some(g) = spans.iter().position(|r| r.contains(&slot)) {
        let line = (slot + usize::from(frac > 0.5)).clamp(spans[g].start, spans[g].end);
        let pos = groups::positions(&st.order);
        let moving: HashSet<u16> = st.sel.iter().map(|&s| st.order[s]).collect();
        let before = st.groups[g]
            .members
            .iter()
            .copied()
            .find(|&x| pos[x as usize] >= line && !moving.contains(&x));
        (SlotDrop::Into { g, before, line }, n_sel)
    } else {
        (SlotDrop::Shift(clear_of(held_top(n_sel), None)), n_sel)
    };
    let (target, hint) = match d {
        SlotDrop::Into { line, .. } => (DropTarget::Insert(line), "\n⌥ swap in place instead"),
        SlotDrop::Place(t) => (DropTarget::Place(t), "\nRelease ⌥ to push down instead"),
        SlotDrop::Shift(t) => (DropTarget::Shift(t), "\n⌥ swap in place instead"),
        SlotDrop::Group { top, .. } => (DropTarget::Shift(top), "\n⌥ split it in two instead"),
        SlotDrop::Split { top, .. } => (DropTarget::Shift(top), "\nRelease ⌥ to move the whole group"),
    };
    let (note, deny) = match st.plan(d) {
        Ok(p) => (p.note + hint, false),
        Err(why) => {
            let deny = !why.starts_with("Already");
            (why, deny)
        }
    };
    let span = slot_list::landing_span(target, n);
    let y = |s: usize| rect.top() + (s as f32 - slot as f32) * ROW_H;
    let into_group = matches!(d, SlotDrop::Into { .. });
    ((rect.x_range(), egui::Rangef::new(y(span.start), y(span.end)), target, note, deny, into_group), d)
}

/// Dashed frames around the groups in this bank, with their name on a tab.
/// The tab and the frame's left and right edges grab the whole group.
fn group_frames(
    ui: &mut egui::Ui,
    st: &ElekState,
    bank: std::ops::Range<usize>,
    (s0, r0): (usize, egui::Rect),
    acts: &mut Vec<RowAct>,
) {
    let y = |s: usize| r0.top() + (s as f32 - s0 as f32) * ROW_H;
    let clip = ui.clip_rect();
    for (g, span) in st.group_spans().into_iter().enumerate() {
        if !bank.contains(&span.start) {
            continue;
        }
        let rect = egui::Rect::from_x_y_ranges(r0.x_range(), y(span.start)..=y(span.end)).shrink2(egui::vec2(1.0, 0.5));
        if !clip.intersects(rect) {
            continue;
        }
        let name = &st.groups[g].name;
        let galley = ui.painter().layout_no_wrap(name.clone(), oled_font(TEXT - 1.0), OLED_BG);
        let tag_size = galley.size() + egui::vec2(14.0, 6.0);
        let tag_top = (rect.top() - tag_size.y / 2.0).max(clip.top()).min(rect.bottom() - tag_size.y);
        let tag = egui::Rect::from_min_size(egui::pos2(rect.right() - 12.0 - tag_size.x, tag_top), tag_size);
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
        let dragging = st.group_drag == Some(g);
        // ⌥ held over a group drag: shade the half that a split would move.
        if dragging && ui.input(|i| i.modifiers.alt) && span.len() >= 2 {
            let half = span.len() / 2;
            let moved = span.end - half..span.end;
            let shade = egui::Rect::from_x_y_ranges(r0.x_range(), y(moved.start)..=y(moved.end));
            ui.painter().rect_filled(shade.intersect(clip), 0.0, st.group_color(&st.groups[g].name, false).gamma_multiply(0.35));
        }
        let hot = dragging || [&tag_r, &left, &right].iter().any(|r| r.hovered());
        if hot && !st.slot_drag {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let c = st.group_color(name, hot);
        let pts = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
        let p = ui.painter();
        p.extend(egui::Shape::dashed_line(&pts, Stroke::new(if hot { 2.0 } else { 1.2 }, c), 5.0, 3.0));
        p.rect_filled(tag, 2.0, c);
        p.galley(tag.min + egui::vec2(5.0, 1.0), galley, OLED_BG);
        for r in [&tag_r, &left, &right] {
            if r.drag_started() {
                let py = ui.input(|i| i.pointer.press_origin()).map_or(rect.top(), |p| p.y);
                let row = (((py - y(span.start)) / ROW_H).floor().max(0.0) as usize).min(span.len() - 1);
                let frac = (py - y(span.start + row)) / ROW_H;
                acts.push(RowAct::GroupDragStart(g, span.start + row, frac));
                r.dnd_set_drag_payload(SlotDrag);
            }
        }
        if tag_r.clicked() {
            acts.push(RowAct::SelectGroup(g));
        }
        for r in [&left, &right] {
            if r.clicked() {
                acts.push(RowAct::AlignGroup(g));
            }
            r.clone().on_hover_text(format!(
                "Click: line the other pane up on its \"{name}\" group\nDrag: move the group"
            ));
        }
        if tag_r.double_clicked() {
            acts.push(RowAct::RenameGroup(g));
        }
        let tag_r = tag_r.on_hover_text(format!(
            "Group \"{name}\": {} slot(s), moves as one.\nDrag the name or the frame's side to move it · ⌥ while dropping splits it in two (second half, new group of the same name) · click the frame's side to line the other pane up on its \"{name}\" · drop slots on its rows to add them · drag one out to remove it\nDouble-click to rename · right-click for more",
            span.len()
        ));
        tag_r.context_menu(|ui| group_menu(ui, st, g, acts));
    }
}

pub(super) fn group_menu(ui: &mut egui::Ui, st: &ElekState, g: usize, acts: &mut Vec<RowAct>) {
    if ui.button("Select group's slots").clicked() {
        acts.push(RowAct::SelectGroup(g));
        ui.close();
    }
    if ui.button("Rename group…").clicked() {
        acts.push(RowAct::RenameGroup(g));
        ui.close();
    }
    match st.compactable(g) {
        Ok(n) => {
            if ui
                .button("Compact group")
                .on_hover_text(format!("Drop its {n} empty slot(s): the samples close up at the top, the freed slots stay just below, outside the group"))
                .clicked()
            {
                acts.push(RowAct::CompactGroup(g));
                ui.close();
            }
        }
        Err(why) => {
            ui.add_enabled(false, egui::Button::new("Compact group")).on_disabled_hover_text(why);
        }
    }
    spread_button(ui, st, g, acts);
    gather_button(ui, st, g, acts);
    if ui.button("Dissolve group").on_hover_text("Forget the group; its samples stay where they are").clicked() {
        acts.push(RowAct::DissolveGroup(g));
        ui.close();
    }
    remove_empty_button(ui, st, acts);
    ui.separator();
    hue_slider(ui, st, g, acts);
}

/// "Remove N empty groups": every group in the project with no samples.
pub(super) fn remove_empty_button(ui: &mut egui::Ui, st: &ElekState, acts: &mut Vec<RowAct>) {
    let empty = st.empty_groups();
    if empty > 0 {
        let label = if empty == 1 { "Remove empty group".to_owned() } else { format!("Remove {empty} empty groups") };
        if ui
            .button(label)
            .on_hover_text("Forget the groups that hold no samples. Slots don't move")
            .clicked()
        {
            acts.push(RowAct::RemoveEmptyGroups);
            ui.close();
        }
    }
}

/// Colour of every group with this one's name: a hue offset from the device
/// yellow, -1..1 (±180°). Double-click the slider or Reset to clear it.
pub(super) fn hue_slider(ui: &mut egui::Ui, st: &ElekState, g: usize, acts: &mut Vec<RowAct>) {
    let Some(name) = st.groups.get(g).map(|gr| gr.name.clone()) else { return };
    let mut h = st.group_hues.get(&groups::hue_key(&name)).copied().unwrap_or(0.0);
    let was = h;
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
        ui.painter().rect_filled(swatch, 2.0, st.group_color(&name, false));
        let resp = ui
            .add(egui::Slider::new(&mut h, -1.0..=1.0).text("Hue").fixed_decimals(2))
            .on_hover_text(format!("Colour of every group named \"{name}\", in all banks and projects\nDouble-click to reset"));
        if resp.double_clicked() {
            h = 0.0;
        }
        if was != 0.0 && ui.small_button("Reset").clicked() {
            h = 0.0;
        }
    });
    if h != was {
        acts.push(RowAct::GroupHue(groups::hue_key(&name), h));
    }
}

/// "Add empty "fx" to other banks": an empty container like this group in
/// every bank that has none, on the same slot numbers when they're free.
/// "Move toward the other banks": where same-named groups elsewhere keep
/// their samples (`ElekState::plan_gather`), disabled with the reason if none.
fn gather_button(ui: &mut egui::Ui, st: &ElekState, g: usize, acts: &mut Vec<RowAct>) {
    let Some(name) = st.groups.get(g).map(|gr| gr.name.clone()) else { return };
    let label = format!("Move toward \"{name}\" in other banks");
    match st.plan_gather(g) {
        Ok(plan) => {
            let tip = format!("{}\nThe averaged slot offsets of the other banks' samples; slots in this bank move out of the way", plan.note);
            if ui.button(label).on_hover_text(tip).clicked() {
                acts.push(RowAct::GatherGroup(g));
                ui.close();
            }
        }
        Err(why) => {
            ui.add_enabled(false, egui::Button::new(label)).on_disabled_hover_text(why);
        }
    }
}

pub(super) fn spread_button(ui: &mut egui::Ui, st: &ElekState, g: usize, acts: &mut Vec<RowAct>) {
    let Some(name) = st.groups.get(g).map(|gr| gr.name.clone()) else { return };
    let targets = st.spread_targets(g);
    let label = format!("Add empty \"{name}\" to other banks");
    let letters = |f: &dyn Fn(Option<usize>) -> bool| {
        targets.iter().filter(|(_, at)| f(*at)).map(|(b, _)| ((b'A' + *b as u8) as char).to_string()).collect::<Vec<_>>().join(", ")
    };
    let fits = letters(&|at| at.is_some());
    let full = letters(&|at| at.is_none());
    if fits.is_empty() {
        let why = if targets.is_empty() {
            format!("Every bank already has a \"{name}\" group")
        } else {
            format!("No bank without \"{name}\" has an empty slot ({full})")
        };
        ui.add_enabled(false, egui::Button::new(label)).on_disabled_hover_text(why);
        return;
    }
    let mut tip = format!("Make a one-slot empty \"{name}\" group in {fits}, on the slot number in the middle of this group where it's empty, else the nearest empty slot.\nDrop samples on it to fill it.");
    if !full.is_empty() {
        tip += &format!("\nNo room in {full}");
    }
    if ui.button(label).on_hover_text(tip).clicked() {
        acts.push(RowAct::SpreadGroup(g));
        ui.close();
    }
}

/// One slot row, laid out like the device's file list.
fn slot_row(ui: &mut egui::Ui, st: &ElekState, slot: usize, acts: &mut Vec<RowAct>, landing: &mut Option<Landing>) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), Sense::empty());
    if st.project.is_none() {
        return rect;
    }
    let from = st.order[slot] as usize;
    let e = st.entry(from);
    let filling = st.filling.get(&(from as u16));
    let added = st.fills.contains_key(&(from as u16)) || filling.is_some();
    let off = slot == 0;
    let resp = ui.interact(
        rect,
        ui.scope_id().with(("slot", slot)),
        if off {
            Sense::hover()
        } else {
            Sense::click_and_drag()
        },
    );
    let selected = st.sel.contains(&slot);
    // A lone row outside a group keeps the inverted highlight; several rows,
    // or a row inside a group, get a grey band behind the light text.
    let inverted = selected && st.sel.len() == 1 && st.group_at(slot).is_none();
    let moved = from != slot || added;

    let painter = ui.painter();
    if inverted {
        painter.rect_filled(rect, 0.0, OLED);
    } else if selected {
        painter.rect_filled(rect, 0.0, SEL_GREY);
    } else if resp.hovered() && !off {
        painter.rect_filled(rect, 0.0, OLED_HOVER);
    }
    if st.cursor == Some(slot) && !selected {
        painter.rect_stroke(
            rect.shrink(0.5),
            0.0,
            Stroke::new(1.0, OLED_DIM),
            egui::StrokeKind::Inside,
        );
    }
    if let Some((_, at)) = st.ping.filter(|&(pos, _)| pos == slot) {
        crate::ui::file_table::paint_ping(ui, rect, at, theme::DEVICE_ACCENT_BRIGHT);
    }
    let painter = ui.painter();
    if moved {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            0.0,
            theme::DEVICE_ACCENT_BRIGHT,
        );
    }
    let (fg, dim) = if inverted {
        (OLED_BG, Color32::from_rgb(60, 70, 66))
    } else {
        (OLED, OLED_DIM)
    };
    // Rows inside a group are smaller and darker; their size is left out.
    let grouped = !off && st.group_at(slot).is_some();
    let font = oled_font(if grouped { TEXT - 2.0 } else { TEXT });
    let y = rect.center().y;
    let label_color = if moved && !selected {
        theme::DEVICE_ACCENT_BRIGHT
    } else {
        dim
    };
    let label_w = 46.0;
    painter.text(
        egui::pos2(rect.left() + 8.0, y),
        egui::Align2::LEFT_CENTER,
        format!("{:03}", slot % BANK),
        font.clone(),
        label_color,
    );
    let name = if off {
        "OFF".to_owned()
    } else if let Some(n) = filling {
        format!("+ {n} …")
    } else if e.is_empty() {
        "-".to_owned()
    } else {
        match (st.names.get(&e.key()), st.linked.get(&e.key())) {
            (Some(path), _) => path.rsplit('/').next().unwrap_or(path).to_owned(),
            // Known from our own transfers while the device lookup runs.
            (None, Some((name, ..))) => name.clone(),
            (None, None) => format!("#{:08x}", e.hash),
        }
    };
    let linked = (!off && !e.is_empty()).then(|| st.linked.get(&e.key())).flatten();
    let uses = st.uses(from);
    let size = if e.is_empty() || off || grouped {
        String::new()
    } else {
        format!("{:.2}MB", e.size.saturating_sub(16) as f64 / 1_048_576.0)
    };
    let size_w = if grouped { 0.0 } else { 58.0 };
    let uses_w = 30.0;
    let name_x = rect.left() + 8.0 + label_w;
    // The sample moved and patterns point at it: the push renumbers them.
    let dot_c = if inverted { OLED_BG } else { theme::DEVICE_ACCENT_BRIGHT };
    if !off && from != slot && uses > 0 {
        painter.circle_filled(egui::pos2(name_x - 7.0, y), 2.5, dot_c);
    }
    // Name brightness shows the preview cache: full white plays at once,
    // 0.8 opacity still has to be pulled from the device.
    let changed = (!off && !e.is_empty()).then(|| st.changed_local.get(&e.key())).flatten();
    // Cut and waiting for a paste: faded, like cut files.
    let is_cut = st.clip.as_ref().is_some_and(|c| c.cut && !e.is_empty() && c.olds.contains(&(from as u16)));
    let name_c = if inverted {
        fg
    } else if is_cut {
        OLED_DIM
    } else if changed.is_some() {
        theme::CHANGED
    } else if linked.is_some() {
        Color32::WHITE
    } else {
        fg.gamma_multiply(0.8)
    };
    let name_c = if grouped && !inverted { name_c.gamma_multiply(0.6) } else { name_c };
    let clip = egui::Rect::from_min_max(
        egui::pos2(name_x, rect.top()),
        egui::pos2(rect.right() - size_w - uses_w, rect.bottom()),
    );
    painter.with_clip_rect(clip).text(
        egui::pos2(name_x, y),
        egui::Align2::LEFT_CENTER,
        name,
        font.clone(),
        if (e.is_empty() && filling.is_none()) || off { dim } else { name_c },
    );
    if uses > 0 {
        painter.text(
            egui::pos2(rect.right() - size_w - 4.0, y),
            egui::Align2::RIGHT_CENTER,
            uses.to_string(),
            oled_font(TEXT - 4.0),
            dim,
        );
    }
    painter.text(
        egui::pos2(rect.right() - 8.0, y),
        egui::Align2::RIGHT_CENTER,
        size,
        font,
        fg,
    );

    if off {
        slot_drop(ui, st, rect, slot, &resp, acts, landing);
        return rect;
    }
    let info = SlotRowInfo {
        slot,
        from,
        entry: e,
        linked,
        changed: changed.is_some(),
        selected,
        moved,
        uses,
        rect,
    };
    let resp = match changed {
        Some(local) => resp.on_hover_text(format!(
            "Edited since it went to the device: the device still plays the older version.\n{}\nRight-click › Update on device from edited file",
            local.display()
        )),
        None => resp,
    };
    // With preview / open on click, the first click of a double-click already
    // played or opened it.
    let on_click = st.preview || st.open_on_click;
    if resp.clicked() && !(on_click && resp.double_clicked()) {
        acts.push(RowAct::Click(slot, ui.input(|i| i.modifiers)));
    }
    if resp.double_clicked() && !on_click && !e.is_empty() {
        acts.push(RowAct::Local(e.key(), false));
    }
    if resp.drag_started() {
        let frac = ui
            .input(|i| i.pointer.press_origin())
            .map_or(0.5, |p| (p.y - rect.top()) / rect.height());
        acts.push(RowAct::DragStart(slot, frac));
        resp.dnd_set_drag_payload(SlotDrag);
    }
    slot_file_drop(ui, st, &info, &resp, acts);
    slot_drop(ui, st, rect, slot, &resp, acts, landing);

    resp.context_menu(|ui| slot_menu(ui, st, &info, acts));
    if !e.is_empty() {
        resp.on_hover_text(slot_tooltip(st, &info));
    } else if filling.is_some() {
        resp.on_hover_text("Uploading to the +Drive, then checking the device can find it. Pushing waits for this.");
    }
    rect
}

/// A slot or group drag over this row; see `slot_landing` for where it lands.
fn slot_drop(
    ui: &egui::Ui,
    st: &ElekState,
    rect: egui::Rect,
    slot: usize,
    resp: &egui::Response,
    acts: &mut Vec<RowAct>,
    landing: &mut Option<Landing>,
) {
    let over = resp.dnd_hover_payload::<SlotDrag>().is_some() || (st.slot_drag && resp.contains_pointer());
    if !over {
        return;
    }
    let (l, d) = slot_landing(ui, st, rect, slot);
    *landing = Some(l);
    // Our own drag flag too: if anything else cleared egui's payload this
    // frame, the drop still lands.
    let released = resp.contains_pointer() && ui.input(|i| i.pointer.any_released());
    if resp.release_payload::<SlotDrag>().is_some() || (released && st.slot_drag) {
        acts.push(RowAct::Drop(d));
    }
}

/// Local files from a file pane (or Finder) onto an empty slot.
fn slot_file_drop(ui: &mut egui::Ui, st: &ElekState, row: &SlotRowInfo<'_>, resp: &egui::Response, acts: &mut Vec<RowAct>) {
    let SlotRowInfo { slot, from, rect, .. } = *row;
    if let Some(d) = resp.dnd_hover_payload::<crate::ui::file_table::DragPayload>().filter(|d| !d.device) {
        let free = st.free(from as u16);
        let held: Vec<usize> = d.paths.iter().filter_map(|p| st.slot_of_local(p)).collect();
        let fresh = d.paths.len() - held.len();
        let ok = free && fresh > 0;
        let n = if ok { fresh.min(st.order.len() - slot) } else { 1 };
        let r = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), ROW_H * n as f32));
        let stroke = if ok { Stroke::new(2.0, theme::DEVICE_ACCENT_BRIGHT) } else { theme::deny_frame() };
        ui.painter().rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
        let note = match (free, held.as_slice()) {
            (false, _) => Some("Drop onto an empty slot (\"-\")".to_owned()),
            (true, [one]) if fresh == 0 => Some(format!("Already in the pool (slot {})", st.label(*one))),
            (true, _) if fresh == 0 => Some(format!("All {} already in the pool", held.len())),
            (true, []) => None,
            (true, _) => Some(format!("{} already in the pool, left out", held.len())),
        };
        if let Some(msg) = note {
            theme::pointer_note(ui.ctx(), &msg, if ok { theme::DEVICE_ACCENT } else { theme::DENY });
        }
    }
    if let Some(d) = resp.release_payload::<crate::ui::file_table::DragPayload>().filter(|d| !d.device) {
        acts.push(RowAct::DropFiles(slot, d.paths.clone()));
    }
}
