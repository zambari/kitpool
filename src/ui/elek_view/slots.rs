//! Sample-pool bank panes, slot drawing and drag/drop action collection.

use super::*;
use crate::ui::slot_groups::ui as sg_ui;
use groups::GroupHost;

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
    let bank_groups = sg_ui::bank_groups(&st.group_view(), bank);
    let nav_h = if bank_groups.is_empty() { 0.0 } else { sg_ui::NAV_H };
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
                ui.set_max_width((ui.available_width() - bar - sg_ui::MINIMAP_W).max(0.0));
                let mut landing = None;
                let mut top_row = None;
                for i in range {
                    let r = slot_row(ui, st, first + i, &mut acts, &mut landing);
                    top_row.get_or_insert((first + i, r));
                }
                if let Some((s0, r0)) = top_row {
                    let y = |s: usize| r0.top() + (s as f32 - s0 as f32) * ROW_H;
                    let mut gacts = Vec::new();
                    sg_ui::frames(ui, &st.group_view(), first..first + rows, r0.x_range(), r0.right(), &y, &|r| r.dnd_set_drag_payload(SlotDrag), &mut gacts);
                    acts.extend(gacts.into_iter().map(RowAct::Group));
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
            sg_ui::minimap(ui, &st.group_view(), first..first + rows, shown.inner_rect, bar);
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
        let gacts = sg_ui::nav_row(
            ui,
            &st.group_view(),
            side,
            &bank_groups,
            &|r| r.dnd_hover_payload::<SlotDrag>().is_some(),
            &|r| r.dnd_set_drag_payload(SlotDrag),
        );
        acts.extend(gacts.into_iter().map(RowAct::Group));
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
        let edits = match &a {
            RowAct::Group(g) => g.edits(),
            a => !matches!(
                a,
                RowAct::Click(..) | RowAct::DragStart(..) | RowAct::Local(..) | RowAct::ShowOriginal(_) | RowAct::Rename(_) | RowAct::Fetch(_) | RowAct::Clip(..) | RowAct::DropClip
            ),
        };
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
            RowAct::Drop(d) => {
                st.active = side;
                st.apply_drop(d);
            }
            RowAct::Group(a) => st.group_act(a, side),
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

/// Drop feedback to paint once the rows are drawn: `(row x range, landing
/// y range, target, pointer note, refused, into a group)`, and the drop itself.
/// Into a group the area is tinted without the insertion line.
type Landing = (egui::Rangef, egui::Rangef, DropTarget, String, bool, bool);

/// Where a slot drag over `slot` lands (`sg_ui::landing`), as feedback.
fn slot_landing(ui: &egui::Ui, st: &ElekState, rect: egui::Rect, slot: usize) -> (Landing, groups::SlotDrop) {
    let rules = slot_list::DropRules { first: 1, len: st.order.len(), plain: slot_list::PlainDrop::Shift };
    let hit = slot_list::RowHit { rect, pitch: ROW_H, row: slot, slot_of: &|r| r };
    let l = sg_ui::landing(ui, &st.group_view(), &hit, &rules);
    let span = slot_list::landing_span(l.target, l.n);
    let y = |s: usize| rect.top() + (s as f32 - slot as f32) * ROW_H;
    ((rect.x_range(), egui::Rangef::new(y(span.start), y(span.end)), l.target, l.note, l.deny, l.into_group), l.drop)
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
