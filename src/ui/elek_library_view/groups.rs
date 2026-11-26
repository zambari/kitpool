//! Sound-pool adapter for the shared slot/group planner. Groups are local
//! per device; cells and groups share one undo snapshot.
use super::*;
use crate::ui::slot_groups::{self, Group, GroupHost, Plan, Slots};

impl LibraryView {
    pub fn set_saved_groups(&mut self, unit: String, saved: Vec<crate::state::SoundGroup>) {
        self.groups_unit = Some(unit);
        let mut used = BTreeSet::new();
        self.groups = saved.into_iter().filter_map(|g| {
            let a = g.at as usize;
            let b = a + g.len as usize;
            if a >= b || b > SOUND_POOL || (a..b).any(|i| used.contains(&i)) { return None; }
            used.extend(a..b);
            Some(Group { name: g.name, members: (a..b).map(|i| i as u16).collect() })
        }).collect();
        self.groups_dirty = false;
    }

    pub fn saved_groups(&self) -> Vec<crate::state::SoundGroup> {
        self.groups.iter().filter_map(|g| Some(crate::state::SoundGroup {
            name: g.name.clone(), at: *g.members.iter().min()?, len: g.members.len() as u16,
        })).collect()
    }

    pub(super) fn group_view<'a>(&'a self, hues: &'a std::collections::BTreeMap<String, f32>) -> slot_groups::ui::GroupView<'a> {
        slot_groups::ui::GroupView { s: self.slots(), hues, drag: None, dragging: false, grab: Default::default(), pitch: ROW_H }
    }

    pub(super) fn snapshot(&mut self) {
        self.sound_undo.push((self.sounds.clone(), self.groups.clone(), self.sel.clone(), self.selected));
    }
}

impl GroupHost for LibraryView {
    fn slots(&self) -> Slots<'_> {
        Slots {
            order: &self.ident, groups: &self.groups, sel: &self.sel,
            bank: SOUND_POOL, first: 0, insert: false, noun: "sounds",
            free_fn: Box::new(|id| self.free.get(id as usize).copied().unwrap_or(false)),
            label_fn: Box::new(|i| format!("{:03}", i + 1)),
            bank_fn: Box::new(|_| "Pool".into()),
        }
    }

    fn commit_plan(&mut self, p: Plan, select: Option<usize>, _: Option<bool>) {
        let old = self.cells_mut().clone();
        self.sounds = p.order.iter().map(|&i| old[i as usize].clone()).collect();
        let positions = slot_groups::positions(&p.order);
        self.groups = p.groups.iter().map(|g| Group {
            name: g.name.clone(), members: g.members.iter().map(|&i| positions[i as usize] as u16).collect(),
        }).collect();
        self.sel = self.sel.iter().map(|&i| positions[i]).collect();
        self.selected = self.selected.map(|i| positions[i]);
        self.free = p.order.iter().map(|&i| self.free[i as usize]).collect();
        if let Some(g) = select { self.select_group(g); }
        self.groups_dirty = true;
        self.note(p.note);
    }
    fn note(&mut self, msg: String) { self.group_note = Some(msg); }
    fn select_group(&mut self, g: usize) {
        if let Some(g) = self.groups.get(g) {
            self.sel = g.members.iter().map(|&i| i as usize).collect();
            self.selected = self.sel.first().copied();
            self.scroll_to = self.selected;
        }
    }
    fn go_to_group(&mut self, g: usize, _: usize) { self.select_group(g); }
    fn align_group(&mut self, g: usize, _: usize) { self.select_group(g); }
    fn group_drag_start(&mut self, g: usize, _: usize, _: f32, _: usize) { self.select_group(g); }
    fn end_group_drag(&mut self) {}
    fn ask_group_name(&mut self, g: Option<usize>, slots: Vec<usize>, name: String) {
        self.group_naming = Some((g, slots, name, true));
    }
    fn set_hue(&mut self, key: String, h: f32) { self.hue_edits.push((key, h)); }
    fn set_active(&mut self, _: usize) {}
}
