//! Sample groups on the RAM page: the shared group engine
//! (`crate::ui::slot_groups`) over the sample pool's order, plus what only
//! samples do: saving per unit by +Drive hash (`state::PoolGroup`), so a
//! project with the same samples gets its groups back, and lining the two
//! panes up.

use super::*;
pub use crate::state::PoolGroup;
pub use crate::ui::slot_groups::{hue_key, Group, GroupAct, GroupHost, Plan, SlotDrop, Slots};
use crate::ui::slot_groups::ui::GroupView;
use std::ops::Range;

impl ElekState {
    pub(super) fn group_view(&self) -> GroupView<'_> {
        GroupView {
            s: self.slots(),
            hues: &self.group_hues,
            drag: self.group_drag,
            dragging: self.slot_drag,
            grab: self.slot_grab,
            pitch: ROW_H,
        }
    }

    /// Each group's slots now, by index.
    pub(super) fn group_spans(&self) -> Vec<Range<usize>> {
        self.slots().group_spans()
    }

    /// The group holding slot `slot`, if any.
    pub(super) fn group_at(&self, slot: usize) -> Option<usize> {
        self.slots().group_at(slot)
    }

    /// True when the selection is exactly group `g`'s slots.
    pub(super) fn group_selected(&self, g: usize) -> bool {
        self.slots().group_selected(g)
    }

    pub(super) fn repair_groups(&self, order: &[u16]) -> Vec<Group> {
        self.slots().repair_groups(order)
    }

    /// The saved groups for this unit, from the app (on start and when
    /// another unit is shown).
    pub fn set_saved_groups(&mut self, unit: String, saved: Vec<PoolGroup>) {
        self.groups_unit = Some(unit);
        self.saved_groups = saved;
        self.match_groups();
    }

    /// Saved groups whose samples sit packed in one bank of the project get
    /// shown; the others stay saved (another project's).
    pub(super) fn match_groups(&mut self) {
        self.groups.clear();
        self.groups_from.clear();
        if self.project.is_none() {
            return;
        }
        let n = self.order.len();
        let mut by_key: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for p in 1..n {
            if let Some(k) = self.key_at(p) {
                by_key.entry(k).or_default().push(p);
            }
        }
        let mut taken: HashSet<usize> = HashSet::new();
        for (i, sg) in self.saved_groups.iter().enumerate() {
            let len = sg.keys.len();
            // Where the group could start: lined up on its first sample, or
            // where it was saved when it holds only empty slots.
            let starts: Vec<usize> = match sg.keys.iter().position(|&k| k != EMPTY_KEY) {
                Some(j) => by_key.get(&sg.keys[j]).map_or(Vec::new(), |v| v.iter().filter_map(|&p| p.checked_sub(j)).collect()),
                None => sg.at.map(|a| a as usize).into_iter().collect(),
            };
            let fits = |a: usize| {
                a >= 1
                    && len > 0
                    && a + len <= n
                    && a / BANK == (a + len - 1) / BANK
                    && sg.keys.iter().enumerate().all(|(j, &k)| {
                        let p = a + j;
                        !taken.contains(&p)
                            && if k == EMPTY_KEY { self.free(self.order[p]) } else { self.key_at(p) == Some(k) }
                    })
            };
            let Some(a) = starts.into_iter().find(|&a| fits(a)) else { continue };
            taken.extend(a..a + len);
            self.groups.push(Group { name: sg.name.clone(), members: self.order[a..a + len].to_vec() });
            self.groups_from.push(i);
        }
    }

    /// Writes the shown groups back into the saved list (by +Drive hash),
    /// replacing the saved ones they came from.
    pub(super) fn sync_saved_groups(&mut self) {
        if self.project.is_none() || self.groups_unit.is_none() {
            return;
        }
        let mut kept: Vec<PoolGroup> = self
            .saved_groups
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.groups_from.contains(i))
            .map(|(_, g)| g.clone())
            .collect();
        let start = kept.len();
        let spans = self.group_spans();
        kept.extend(self.groups.iter().zip(&spans).map(|(g, span)| PoolGroup {
            name: g.name.clone(),
            keys: g.members.iter().map(|&x| if self.free(x) { EMPTY_KEY } else { self.entry(x as usize).key() }).collect(),
            at: Some(span.start as u16),
        }));
        self.groups_from = (start..kept.len()).collect();
        if kept != self.saved_groups {
            self.saved_groups = kept;
            self.groups_dirty = true;
        }
    }
}

impl GroupHost for ElekState {
    fn slots(&self) -> Slots<'_> {
        Slots {
            order: &self.order,
            groups: &self.groups,
            sel: &self.sel,
            bank: BANK,
            first: 1,
            insert: false,
            noun: "samples",
            free_fn: Box::new(move |id| self.free(id)),
            label_fn: Box::new(move |s| self.label(s)),
            bank_fn: Box::new(|b| ((b'A' + b as u8) as char).to_string()),
        }
    }

    fn commit_plan(&mut self, p: Plan, select: Option<usize>, lead: Option<bool>) {
        self.commit(p.order, p.groups);
        if let Some(g) = select {
            self.select_group(g);
            if let (Some(down), Some(span)) = (lead, self.group_spans().get(g).cloned()) {
                let at = if down { span.end - 1 } else { span.start };
                self.cursor = Some(at);
                self.reveal = Some(at);
            }
        }
        self.status = p.note;
    }

    fn note(&mut self, msg: String) {
        self.status = msg;
    }

    fn select_group(&mut self, g: usize) {
        let Some(span) = self.group_spans().get(g).cloned() else { return };
        self.sel = span.clone().collect();
        self.cursor = Some(span.start);
        self.group_held = Some(g);
    }

    /// Jumps to group `g` from pane `side`: the pane opens its bank and, when
    /// the group isn't all in view, eases (via `align[side]`) to show it from
    /// one row above its first slot. Its first slot flashes and the group is
    /// selected. (Not `ping_scroll`: the jump row draws after the list, and
    /// the pane clears that flag the same frame.)
    fn go_to_group(&mut self, g: usize, side: usize) {
        let Some(span) = self.group_spans().get(g).cloned() else { return };
        self.select_group(g);
        self.active = side;
        self.ping = Some((span.start, std::time::Instant::now()));
        let bank = span.start / BANK;
        let rows = BANK.min(self.order.len() - bank * BANK);
        let (top, h) = (self.scroll[side], self.view_h[side]);
        let (a, b) = ((span.start % BANK) as f32 * ROW_H, ((span.end - 1) % BANK + 1) as f32 * ROW_H);
        if self.bank[side] != bank || a < top || b > top + h {
            self.bank[side] = bank;
            let max = (rows as f32 * ROW_H - h).max(0.0);
            self.align[side] = Some((a - ROW_H).clamp(0.0, max));
        }
    }

    /// Group `g` was clicked by its frame in pane `side`: scroll the other
    /// pane so its group of the same name (in the bank it shows) sits level
    /// with this one. When the other pane can't scroll that far, this pane
    /// moves to meet it.
    fn align_group(&mut self, g: usize, side: usize) {
        if self.scroll_linked {
            self.status = "The panes are scroll-linked: unlock the padlock between them to line groups up".into();
            return;
        }
        let spans = self.group_spans();
        let (Some(gr), Some(span)) = (self.groups.get(g), spans.get(g).cloned()) else { return };
        let name = hue_key(&gr.name);
        let other = 1 - side;
        let n = self.order.len();
        let bank_o = self.bank[other];
        let same: Vec<usize> = (0..self.groups.len()).filter(|&h| h != g && hue_key(&self.groups[h].name) == name).collect();
        let letter = |b: usize| (b'A' + b as u8) as char;
        let Some(h) = same.iter().copied().find(|&h| spans[h].start / BANK == bank_o) else {
            let mut elsewhere: Vec<char> = same.iter().map(|&h| letter(spans[h].start / BANK)).collect();
            elsewhere.dedup();
            self.status = if elsewhere.is_empty() {
                format!("No other group named \"{}\" to line up with", gr.name)
            } else {
                let list: Vec<String> = elsewhere.iter().map(char::to_string).collect();
                format!("No \"{}\" group in bank {}; there's one in {}", gr.name, letter(bank_o), list.join(", "))
            };
            return;
        };
        let max = |b: usize, s: usize| ((BANK.min(n - b * BANK)) as f32 * ROW_H - self.view_h[s]).max(0.0);
        let top = |r: &Range<usize>| (r.start % BANK) as f32 * ROW_H;
        let (top_a, top_b) = (top(&span), top(&spans[h]));
        // Where the clicked group sits in its pane now; the other one goes there.
        let y = top_a - self.scroll[side];
        let o = (top_b - y).clamp(0.0, max(bank_o, other));
        let a = (top_a - top_b + o).clamp(0.0, max(span.start / BANK, side));
        if (o - self.scroll[other]).abs() > 0.5 {
            self.align[other] = Some(o);
        }
        if (a - self.scroll[side]).abs() > 0.5 {
            self.align[side] = Some(a);
        }
        self.status = format!("Lined up \"{}\": {} ↔ {}", gr.name, self.label(span.start), self.label(spans[h].start));
    }

    fn group_drag_start(&mut self, g: usize, slot: usize, frac: f32, side: usize) {
        self.slot_drag = true;
        self.group_drag = Some(g);
        self.active = side;
        self.select_group(g);
        self.slot_grab = slot_list::Grab::new(&self.sel, slot, frac);
    }

    fn end_group_drag(&mut self) {
        self.slot_drag = false;
        self.group_drag = None;
    }

    fn ask_group_name(&mut self, g: Option<usize>, slots: Vec<usize>, name: String) {
        self.group_naming = Some((g, slots, name, true));
    }

    fn set_hue(&mut self, key: String, h: f32) {
        if h == 0.0 {
            self.group_hues.remove(&key);
        } else {
            self.group_hues.insert(key, h);
        }
        self.hues_dirty = true;
    }

    fn set_active(&mut self, side: usize) {
        self.active = side;
    }
}

/// Saved key of an empty slot in a group.
const EMPTY_KEY: (u32, u32) = (0, 0);
