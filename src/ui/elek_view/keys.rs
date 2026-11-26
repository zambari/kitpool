//! Sample-pool keyboard navigation, playback and clipboard dispatch.

use super::*;
use groups::GroupHost;

impl ElekState {
    pub(super) fn keys(&mut self, ctx: &egui::Context) {
        if !self.keys_on || ctx.egui_wants_keyboard_input() || self.project.is_none() {
            return;
        }
        if self.multi_rename.is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F2)) {
            let slots = self.sel.iter().copied().collect::<Vec<_>>();
            self.begin_multi_rename(&slots);
            return;
        }
        let n = self.order.len();
        if self.group_held.is_some_and(|g| !self.group_selected(g)) {
            self.group_held = None;
        }
        let (m, up, down, pgup, pgdn, esc, z, a, space) = ctx.input(|i| {
            (
                i.modifiers,
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Z),
                i.key_pressed(egui::Key::A),
                i.key_pressed(egui::Key::Space),
            )
        });
        // ⌘X / ⌘C / ⌘V come as clipboard events (⌘V only when the system
        // clipboard holds text); ⇧⌘V comes as a plain key.
        let (ev_cut, ev_copy, ev_paste, shift_paste) = ctx.input(|i| {
            let ev = |f: fn(&egui::Event) -> bool| i.events.iter().any(f);
            (
                ev(|e| matches!(e, egui::Event::Cut)),
                ev(|e| matches!(e, egui::Event::Copy)),
                ev(|e| matches!(e, egui::Event::Paste(_))) || (i.modifiers.command_only() && i.key_pressed(egui::Key::V)),
                i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::V),
            )
        });
        if ev_cut || ev_copy {
            let slots: Vec<usize> = self.sel.iter().copied().collect();
            self.set_clip(&slots, ev_cut, ctx);
        }
        if ev_paste || shift_paste {
            if self.edit_locked() {
                self.status = "Wait a moment: the device is busy, and an edit now would be lost".into();
            } else {
                let at = self.cursor.or_else(|| self.sel.first().copied()).unwrap_or(1);
                self.paste(at, shift_paste);
            }
        }
        // Space plays (or stops) the slot under the cursor.
        if space && !m.any() {
            if let Some(k) = self.cursor.and_then(|c| self.key_at(c)) {
                self.want_local = Some((k, false));
            }
        }
        if m.command && z {
            if self.edit_locked() {
                self.status = "Undo waits: the device is busy".into();
            } else {
                self.undo();
            }
        }
        if m.command && a {
            let b = self.bank[self.active] * BANK;
            self.sel = (b.max(1)..(b + BANK).min(n)).collect();
        }
        if esc {
            self.sel.clear();
        }
        let step: i32 = if up {
            -1
        } else if down {
            1
        } else if pgup {
            -16
        } else if pgdn {
            16
        } else {
            0
        };
        if step == 0 {
            return;
        }
        // A whole group selected: arrows move it (⇧ / PgUp / PgDn: up to a
        // page), across banks at the edges.
        if !m.alt && !m.command {
            if let Some(g) = self.group_held {
                if self.edit_locked() {
                    self.status = "Wait a moment: the device is busy, and an edit now would be lost".into();
                } else {
                    let page = m.shift || pgup || pgdn;
                    self.step_group(g, if page { step.signum() * 16 } else { step.signum() });
                }
                return;
            }
        }
        if m.alt && (up || down) {
            if !self.edit_locked() {
                self.nudge(step);
            }
            return;
        }
        let c = self.cursor.unwrap_or(0) as i32;
        let next = (c + step).clamp(1, n as i32 - 1) as usize;
        if m.shift {
            let a = self.cursor.unwrap_or(next);
            self.sel.insert(a);
            self.sel.insert(next);
        } else {
            self.sel.clear();
            self.sel.insert(next);
        }
        self.cursor = Some(next);
        self.reveal = Some(next);
        if self.preview && !m.shift {
            if let Some(k) = self.key_at(next) {
                self.want_local = Some((k, false));
            }
        }
    }
}
