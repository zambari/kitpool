//! Slot groups, shared by the RAM (sample pool) and Presets pages: named runs
//! of adjacent slots in one bank that move as one unit. Only this app knows
//! them (the device has no such thing). Each page saves its own (samples by
//! +Drive hash, presets by bank and slot).
//!
//! Rules: a group is a packed run of slots inside one bank. It may hold
//! empty slots (a *container*: places waiting to be filled). Dropping slots
//! into a group fills its empty slots below the drop line first, else adds
//! them and pushes what follows down as far as the next free slot; dragging
//! one out closes the group up and pulls up what follows, as far as the
//! next free slot, and a container then takes free slots next to it back
//! up to its old size. An empty slot in a group is never "free" for a push
//! from outside it: it moves with its group. Every slot move is worked
//! out first (`Slots::plan`), so the pointer note says what will happen
//! and a move that would break a group is refused before it happens.
//!
//! The planning works on a page's slot order (`order[slot] = id`) through
//! [`Slots`]; the page applies a [`Plan`] its own way ([`GroupHost`]). The
//! drawing (frames, minimap, jump row, menus) is in [`ui`].

use crate::ui::slot_list;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;

pub mod ui;

#[derive(Clone, PartialEq, Debug)]
pub struct Group {
    pub name: String,
    /// Ids (`order` entries) in the group, top to bottom. Always adjacent
    /// slots in one bank.
    pub members: Vec<u16>,
}

/// What a slot drop does, before it happens.
#[derive(Clone, Copy)]
pub enum SlotDrop {
    /// The selection lands at `top`, pushing what's there down to the next
    /// empty slot (inserting, on a page that inserts: `Slots::insert`).
    Shift(usize),
    /// The selection joins group `g` before its member `before` (`None`: at
    /// its end). `line` is the row boundary, for the feedback.
    Into { g: usize, before: Option<u16>, line: usize },
    /// All of group `g` lands at `top`, pushing down like `Shift`.
    Group { g: usize, top: usize },
    /// ⌥ with a group: its second half lands at `top` as a new group of the
    /// same name, the first half stays (`plan_split`).
    Split { g: usize, top: usize },
    /// ⌥: the selection swaps in place.
    Place(usize),
}

/// How hard a group reorder (`Slots::plan_reorder`) may shuffle its
/// bank, gentlest first. Each level allows the ones before it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Shuffle {
    /// Only into free slots already there: nothing else moves.
    Fit,
    /// Push neighbours aside, each only as far as the next free slot.
    Push,
    /// Slide everything between the group and its new spot over by the
    /// group's length. Always possible inside a bank.
    Rotate,
}

/// A worked-out edit: the new order and groups, and what to say about it.
pub struct Plan {
    pub order: Vec<u16>,
    pub groups: Vec<Group>,
    pub note: String,
}

/// `pos[id] = position now`.
pub fn positions(order: &[u16]) -> Vec<usize> {
    let mut pos = vec![0; order.len()];
    for (i, &o) in order.iter().enumerate() {
        pos[o as usize] = i;
    }
    pos
}

/// The `group_hues` key for groups named `name`: names match ignoring case
/// and surrounding spaces, as everywhere else groups compare names.
pub fn hue_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The colour of groups named `name`: the device accent (`bright` when
/// hot) turned by its hue offset. Hues are app-wide, both pages.
pub fn group_color(hues: &BTreeMap<String, f32>, name: &str, bright: bool) -> egui::Color32 {
    use crate::ui::theme;
    let base = if bright { theme::DEVICE_ACCENT_BRIGHT } else { theme::DEVICE_ACCENT };
    theme::hue_shift(base, hues.get(&hue_key(name)).copied().unwrap_or(0.0))
}

/// A page's slots as the group planner sees them.
pub struct Slots<'a> {
    /// `order[slot] = id`.
    pub order: &'a [u16],
    pub groups: &'a [Group],
    pub sel: &'a BTreeSet<usize>,
    /// Slots per bank.
    pub bank: usize,
    /// First slot anything may move to (the sample pool's 000 is "off").
    pub first: usize,
    /// Plain drops insert (slots in between shift, empty ones included)
    /// rather than push down only as far as the next empty slot.
    pub insert: bool,
    /// What the slots hold, plural ("samples", "presets"), for the notes.
    pub noun: &'static str,
    pub free_fn: Box<dyn Fn(u16) -> bool + 'a>,
    pub label_fn: Box<dyn Fn(usize) -> String + 'a>,
    pub bank_fn: Box<dyn Fn(usize) -> String + 'a>,
}

impl Slots<'_> {
    /// Id `id` holds nothing.
    pub fn free(&self, id: u16) -> bool {
        (self.free_fn)(id)
    }

    /// Slot as shown (`A012`).
    pub fn label(&self, slot: usize) -> String {
        (self.label_fn)(slot)
    }

    /// Bank `b`'s name (`A`).
    pub fn bname(&self, b: usize) -> String {
        (self.bank_fn)(b)
    }

    fn bank_end(&self, p: usize) -> usize {
        ((p / self.bank + 1) * self.bank).min(self.order.len())
    }

    fn range(&self, a: usize, m: usize) -> String {
        if m == 1 { self.label(a) } else { format!("{}–{}", self.label(a), self.label(a + m - 1)) }
    }

    /// Each group's slots now, by index.
    pub fn group_spans(&self) -> Vec<Range<usize>> {
        let pos = positions(self.order);
        self.groups.iter().map(|g| span_of(g, &pos)).collect()
    }

    /// The group holding slot `slot`, if any.
    pub fn group_at(&self, slot: usize) -> Option<usize> {
        let id = *self.order.get(slot)?;
        self.groups.iter().position(|g| g.members.contains(&id))
    }

    /// True when the selection is exactly group `g`'s slots.
    pub fn group_selected(&self, g: usize) -> bool {
        let Some(span) = self.group_spans().get(g).cloned() else { return false };
        self.sel.len() == span.len() && span.clone().all(|s| self.sel.contains(&s))
    }

    /// Slots `pick` can become a group: in one bank, none grouped yet (gaps
    /// are fine: `new_group` packs them). Empty slots are fine (a
    /// container). `Err` says why not.
    pub fn groupable(&self, pick: &[usize]) -> Result<(), String> {
        let (Some(&a), Some(&b)) = (pick.iter().min(), pick.iter().max()) else { return Err("Nothing selected".into()) };
        if a < self.first {
            return Err(format!("Slot {} is \"off\" and can't be grouped", self.label(a)));
        }
        if a / self.bank != b / self.bank {
            return Err("A group stays inside one bank".into());
        }
        if pick.iter().any(|&s| self.group_at(s).is_some()) {
            return Err("Some of these slots are already in a group".into());
        }
        Ok(())
    }

    /// Why a group can't be called `name` in `bank`: another group of that
    /// name already sits in the bank. (Same names in other banks are fine.)
    fn name_taken(&self, name: &str, bank: usize, except: Option<usize>) -> Option<String> {
        let want = hue_key(name);
        let spans = self.group_spans();
        let taken = self
            .groups
            .iter()
            .enumerate()
            .any(|(i, g)| Some(i) != except && spans[i].start / self.bank == bank && hue_key(&g.name) == want);
        taken.then(|| format!("A group named \"{}\" already exists in bank {}", name.trim(), self.bname(bank)))
    }

    /// A new group of slots `pick`, named `name`.
    pub fn new_group(&self, pick: &[usize], name: String) -> Result<Plan, String> {
        self.groupable(pick)?;
        if let Some(&first) = pick.iter().min() {
            if let Some(why) = self.name_taken(&name, first / self.bank, None) {
                return Err(why);
            }
        }
        let mut pick = pick.to_vec();
        pick.sort_unstable();
        pick.dedup();
        // A group with something in it is born compact: only the picked
        // slots that hold something join, packed in order. Only empty slots:
        // a container of all of them.
        let filled: Vec<usize> = pick.iter().copied().filter(|&s| !self.free(self.order[s])).collect();
        let take = if filled.is_empty() { &pick } else { &filled };
        let members: Vec<u16> = take.iter().map(|&s| self.order[s]).collect();
        let m = members.len();
        let count = m;
        // Where the run goes: the spot that moves the picked slots least,
        // using only picked slots and loose empty ones, so nothing else
        // moves; the slots they leave stay behind, empty and ungrouped.
        let bank0 = pick[0] / self.bank * self.bank;
        let lo = bank0.max(self.first);
        let hi = self.bank_end(pick[0]);
        let usable = |p: usize| pick.binary_search(&p).is_ok() || (self.free(self.order[p]) && self.group_at(p).is_none());
        let spot = (lo..=hi.saturating_sub(m))
            .filter(|&s| s + m <= hi && (s..s + m).all(usable))
            .min_by_key(|&s| (take.iter().enumerate().map(|(i, &p)| p.abs_diff(s + i)).sum::<usize>(), s));
        let Some(s) = spot else {
            // No room without moving others: land them like a drop at the
            // first pick, pushing what's there aside.
            let Plan { order, mut groups, note } = self.plan_picked(SlotDrop::Shift(pick[0]), Some(take))?;
            groups.push(Group { name: name.clone(), members });
            self.check_groups(&order, &groups)?;
            return Ok(Plan { order, groups, note: format!("Grouped {count} slot(s) as \"{name}\" · {note}") });
        };
        let mut order = self.order.to_vec();
        let target: Vec<usize> = (s..s + m).collect();
        let tset: BTreeSet<usize> = target.iter().copied().collect();
        let sset: BTreeSet<usize> = take.iter().copied().collect();
        // What sits in the target now (empty slots) goes where the members leave.
        let displaced: Vec<u16> = target.iter().filter(|p| !sset.contains(p)).map(|&p| self.order[p]).collect();
        let vacated: Vec<usize> = take.iter().copied().filter(|p| !tset.contains(p)).collect();
        for (&p, &id) in vacated.iter().zip(&displaced) {
            order[p] = id;
        }
        for (&p, &id) in target.iter().zip(&members) {
            order[p] = id;
        }
        let mut groups = self.groups.to_vec();
        groups.push(Group { name: name.clone(), members });
        Ok(Plan { order, groups, note: format!("Grouped {count} slot(s) as \"{name}\"") })
    }

    pub fn rename_group(&self, g: usize, name: String) -> Result<Plan, String> {
        if let Some(span) = self.group_spans().get(g).cloned() {
            if let Some(why) = self.name_taken(&name, span.start / self.bank, Some(g)) {
                return Err(why);
            }
        }
        let mut groups = self.groups.to_vec();
        let gr = groups.get_mut(g).ok_or("No such group")?;
        gr.name = name.clone();
        Ok(Plan { order: self.order.to_vec(), groups, note: format!("Renamed the group \"{name}\"") })
    }

    /// Groups holding nothing at all (only empty slots, or none).
    pub fn empty_groups(&self) -> usize {
        self.groups.iter().filter(|g| g.members.iter().all(|&x| self.free(x))).count()
    }

    /// Forgets every group with nothing in it. Slots don't move.
    pub fn remove_empty_groups(&self) -> Result<Plan, String> {
        let groups: Vec<Group> = self.groups.iter().filter(|g| !g.members.iter().all(|&x| self.free(x))).cloned().collect();
        let removed = self.groups.len() - groups.len();
        if removed == 0 {
            return Err("No empty groups to remove".into());
        }
        Ok(Plan { order: self.order.to_vec(), groups, note: format!("Removed {removed} empty group(s)") })
    }

    /// Why group `g` can't be compacted, or how many empty slots it would drop.
    pub fn compactable(&self, g: usize) -> Result<usize, &'static str> {
        let gr = self.groups.get(g).ok_or("No such group")?;
        if gr.members.len() < 2 {
            return Err("The group is a single slot");
        }
        let empty = gr.members.iter().filter(|&&x| self.free(x)).count();
        match empty {
            0 => Err("The group has no empty slots"),
            e if e == gr.members.len() => Ok(e - 1),
            e => Ok(e),
        }
    }

    /// Drops group `g`'s empty slots: what it holds closes up at its top (in
    /// order) and the freed slots stay right after it, ungrouped. An all-empty
    /// container shrinks to its first slot. Nothing outside the group moves.
    pub fn compact_group(&self, g: usize) -> Result<Plan, String> {
        self.compactable(g)?;
        let span = self.group_spans()[g].clone();
        let gr = &self.groups[g];
        let (filled, empty): (Vec<u16>, Vec<u16>) = self.order[span.clone()].iter().copied().partition(|&id| !self.free(id));
        let members = if filled.is_empty() { vec![self.order[span.start]] } else { filled.clone() };
        let dropped = gr.members.len() - members.len();
        let name = gr.name.clone();
        let mut order = self.order.to_vec();
        let mut run = filled;
        run.extend(empty);
        order.splice(span, run);
        let mut groups = self.groups.to_vec();
        groups[g].members = members;
        Ok(Plan { order, groups, note: format!("Compacted \"{name}\": dropped {dropped} empty slot(s)") })
    }

    /// Forgets group `g`; its slots stay where they are.
    pub fn dissolve_group(&self, g: usize) -> Result<Plan, String> {
        let mut groups = self.groups.to_vec();
        if g >= groups.len() {
            return Err("No such group".into());
        }
        let gone = groups.remove(g);
        Ok(Plan {
            order: self.order.to_vec(),
            groups,
            note: format!("Dissolved the group \"{}\"; its {} stay where they are", gone.name, self.noun),
        })
    }

    /// For every bank without a group named like `g`: where a one-slot
    /// empty container would go, `(bank, slot)`, or `None` when the bank has
    /// no empty, ungrouped slot. Prefers the slot number in the middle of
    /// `g`, then the nearest one.
    pub fn spread_targets(&self, g: usize) -> Vec<(usize, Option<usize>)> {
        let spans = self.group_spans();
        let (Some(gr), Some(span)) = (self.groups.get(g), spans.get(g)) else { return Vec::new() };
        let name = hue_key(&gr.name);
        let n = self.order.len();
        let off = self.mid_of(span);
        let open = |p: usize| p >= self.first && self.free(self.order[p]) && self.group_at(p).is_none();
        (0..n.div_ceil(self.bank))
            .filter(|&b| b != span.start / self.bank)
            .filter(|&b| !self.groups.iter().zip(&spans).any(|(h, r)| r.start / self.bank == b && hue_key(&h.name) == name))
            .map(|b| {
                let first = b * self.bank;
                let last = (first + self.bank).min(n);
                let at = (first..last).filter(|&a| open(a)).min_by_key(|&a| (a - first).abs_diff(off));
                (b, at)
            })
            .collect()
    }

    /// A one-slot empty group named like `g` in every bank that has none.
    pub fn spread_group(&self, g: usize) -> Result<Plan, String> {
        let gr = self.groups.get(g).ok_or("No such group")?;
        let name = gr.name.clone();
        let off = self.mid_of(&self.group_spans()[g]);
        let targets = self.spread_targets(g);
        if targets.is_empty() {
            return Err(format!("Every bank already has a \"{name}\" group"));
        }
        let mut groups = self.groups.to_vec();
        let (mut same, mut moved, mut none) = (Vec::new(), Vec::new(), Vec::new());
        for &(b, at) in &targets {
            match at {
                Some(a) => {
                    groups.push(Group { name: name.clone(), members: vec![self.order[a]] });
                    if a % self.bank == off { same.push(self.bname(b)) } else { moved.push(self.label(a)) }
                }
                None => none.push(self.bname(b)),
            }
        }
        let mut parts = Vec::new();
        if !same.is_empty() {
            parts.push(format!("in {} at {}", same.join(", "), self.label(off).trim_start_matches(|c: char| !c.is_ascii_digit())));
        }
        if !moved.is_empty() {
            parts.push(format!("at {}", moved.join(", ")));
        }
        let tail = if none.is_empty() { String::new() } else { format!(" · no empty slot in {}", none.join(", ")) };
        if parts.is_empty() {
            return Err(format!("No room for an empty \"{name}\"{tail}"));
        }
        Ok(Plan { order: self.order.to_vec(), groups, note: format!("Made empty \"{name}\" groups {}{tail}", parts.join("; ")) })
    }

    /// Moves group `g` toward where the same-named groups in the other banks
    /// keep their slots: the mean slot offset (within the bank) over their
    /// filled slots. `g` is placed so its middle lands on that mean, or on
    /// the nearest spot its bank allows. Slots in its bank move out of the
    /// way as for a group drop; a spot that would push anything into
    /// another bank is skipped.
    pub fn plan_gather(&self, g: usize) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let spans = self.group_spans();
        let span = spans[g].clone();
        let n = self.order.len();
        let bk = self.bank;
        let own = span.start / bk;
        let want = hue_key(&gr.name);
        let (mut sum, mut count) = (0usize, 0usize);
        let mut banks: Vec<String> = Vec::new();
        for (h, other) in self.groups.iter().enumerate() {
            let b = spans[h].start / bk;
            if h == g || b == own || hue_key(&other.name) != want {
                continue;
            }
            // Filled slots count; a container with none counts by its empty slots.
            let filled: Vec<usize> = spans[h].clone().filter(|&s| !self.free(self.order[s])).collect();
            let slots: Vec<usize> = if filled.is_empty() { spans[h].clone().collect() } else { filled };
            sum += slots.iter().map(|s| s % bk).sum::<usize>();
            count += slots.len();
            banks.push(self.bname(b));
        }
        if count == 0 {
            return Err(format!("No other bank has a \"{}\" group", gr.name));
        }
        let mean = sum as f32 / count as f32;
        let mean_label = || self.label(own * bk + mean.round() as usize);
        let len = span.len();
        let base = own * bk;
        let bank_len = bk.min(n - base);
        if len > bank_len {
            return Err("The group doesn't fit in its bank".into());
        }
        // Where the group's middle should be, as its first in-bank offset.
        let ideal = mean - (len as f32 - 1.0) / 2.0;
        let mut tops: Vec<usize> = (0..=bank_len - len).filter(|&t| base + t >= self.first).collect();
        tops.sort_by(|&a, &b| (a as f32 - ideal).abs().total_cmp(&(b as f32 - ideal).abs()));
        let banks = banks.join(", ");
        for t in tops {
            let top = base + t;
            let Ok(p) = self.plan_picked(SlotDrop::Group { g, top }, None) else { continue };
            if p.order.iter().zip(self.order).enumerate().any(|(i, (new, old))| new != old && i / bk != own) {
                continue;
            }
            if p.order == self.order {
                return Err(format!(
                    "\"{}\" already sits as close to the mean slot ({}) of the other banks as its bank allows",
                    gr.name,
                    mean_label()
                ));
            }
            let mut note = format!(
                "\"{}\" to {}, nearest the mean slot ({}) of the \"{}\" groups in {banks}",
                gr.name,
                self.range(top, len),
                mean_label(),
                gr.name
            );
            if !p.note.is_empty() {
                note += &format!(" · {}", p.note);
            }
            return Ok(Plan { note, ..p });
        }
        Err(format!("No room in its bank to move \"{}\" toward {}", gr.name, mean_label()))
    }

    /// Works out a drop without doing it: `Err` says why it can't happen.
    pub fn plan(&self, d: SlotDrop) -> Result<Plan, String> {
        if let SlotDrop::Split { g, top } = d {
            return self.plan_split(g, top);
        }
        if let SlotDrop::Group { g, top } = d {
            if let Some(h) = self.same_name_in_bank(g, top / self.bank) {
                return self.plan_merge(g, h, top);
            }
        }
        self.plan_picked(d, None)
    }

    /// The group index a plan's groups give group `g` (by a member), for
    /// selecting it after the edit (a merge drops `g`; a split adds one).
    pub fn held_after(&self, d: SlotDrop, p: &Plan) -> Option<usize> {
        match d {
            SlotDrop::Group { g, .. } => {
                let id = self.groups.get(g)?.members.first().copied()?;
                p.groups.iter().position(|gr| gr.members.contains(&id))
            }
            SlotDrop::Split { .. } => p.groups.len().checked_sub(1),
            _ => None,
        }
    }

    /// Another group named like `g` in bank `bank` (never `g` itself).
    fn same_name_in_bank(&self, g: usize, bank: usize) -> Option<usize> {
        let want = hue_key(&self.groups.get(g)?.name);
        let spans = self.group_spans();
        (0..self.groups.len()).find(|&i| i != g && spans[i].start / self.bank == bank && hue_key(&self.groups[i].name) == want)
    }

    /// Group `g` dropped at `top` in a bank that already has its same-named
    /// group `h`: `g`'s slots join `h` (before it when dropped at or above
    /// its top, else at its end) and `g` ends. Two groups of one name never
    /// share a bank.
    fn plan_merge(&self, g: usize, h: usize, top: usize) -> Result<Plan, String> {
        let spans = self.group_spans();
        let pos = positions(self.order);
        let before = if top <= spans[h].start {
            self.groups[h].members.iter().copied().min_by_key(|&x| pos[x as usize])
        } else {
            None
        };
        let picked: Vec<usize> = spans[g].clone().collect();
        let p = self.plan_join_at(h, &picked, before)?;
        let name = &self.groups[g].name;
        let bank = self.bname(spans[h].start / self.bank);
        let rest = p.note.replace(&format!(" · \"{name}\" ends (empty)"), "");
        Ok(Plan { note: format!("Merge \"{name}\" into its namesake in bank {bank} · {rest}"), ..p })
    }

    /// ⌥-drop of group `g` at `top`: the group's second half moves to a new
    /// group of the same name there, the first half stays. Refused when the
    /// bank at `top` already has a group of that name. The new group is a
    /// packed run, so the drop is planned like a `Shift` of those slots.
    fn plan_split(&self, g: usize, top: usize) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let len = gr.members.len();
        if len < 2 {
            return Err(format!("\"{}\" has one slot: nothing to split", gr.name));
        }
        if let Some(why) = self.name_taken(&gr.name, top / self.bank, None) {
            return Err(why);
        }
        let pos = positions(self.order);
        let mut members = gr.members.clone();
        members.sort_by_key(|&x| pos[x as usize]);
        let moved = members.split_off(len - len / 2);
        let picked: Vec<usize> = moved.iter().map(|&x| pos[x as usize]).collect();
        let Plan { order, mut groups, note } = self.plan_picked(SlotDrop::Shift(top), Some(&picked))?;
        groups.push(Group { name: gr.name.clone(), members: moved.clone() });
        self.check_groups(&order, &groups)?;
        Ok(Plan { order, groups, note: format!("Split \"{}\" · {note}", gr.name) })
    }

    /// Moves `pick` into group `g` at its end, as a drop there would (the
    /// slot menu's "Move to group"). Members already in `g` stay put.
    fn plan_join(&self, g: usize, pick: &[usize]) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let n = self.order.len();
        let mut picked: Vec<usize> =
            pick.iter().copied().filter(|&s| s >= self.first && s < n && !gr.members.contains(&self.order[s])).collect();
        picked.sort_unstable();
        picked.dedup();
        if picked.is_empty() {
            return Err(format!("Already all in \"{}\"", gr.name));
        }
        self.plan_picked(SlotDrop::Into { g, before: None, line: 0 }, Some(&picked))
    }

    /// `plan_join`, refused when the shuffle would move a slot into a bank
    /// other than the group's or the ones `pick` came from. Pushes that run
    /// over the bank's edge are caught here, not left to the next bank.
    pub fn plan_join_in_bank(&self, g: usize, pick: &[usize]) -> Result<Plan, String> {
        self.plan_join_at(g, pick, None)
    }

    /// `plan_join_in_bank`, landing before member `before` (`None`: at the end).
    fn plan_join_at(&self, g: usize, pick: &[usize], before: Option<u16>) -> Result<Plan, String> {
        let plan = match before {
            None => self.plan_join(g, pick)?,
            Some(_) => self.plan_picked(SlotDrop::Into { g, before, line: 0 }, Some(pick))?,
        };
        let Some(span) = self.group_spans().get(g).cloned() else { return Err("No such group".into()) };
        let mut allowed: HashSet<usize> = pick.iter().filter(|&&s| s < self.order.len()).map(|&s| s / self.bank).collect();
        allowed.insert(span.start / self.bank);
        let spilled = plan
            .order
            .iter()
            .zip(self.order)
            .enumerate()
            .filter(|(_, (new, old))| new != old)
            .map(|(i, _)| i / self.bank)
            .find(|b| !allowed.contains(b));
        match spilled {
            Some(b) => Err(format!(
                "That would spill into bank {} (the group is in {}); the group's bank has no room for it",
                self.bname(b),
                self.bname(span.start / self.bank)
            )),
            None => Ok(plan),
        }
    }

    /// Reorders group `g` within its bank so it sits at `at` among the bank's
    /// *other* groups in slot order (0: before the first; their count: after
    /// the last). Nothing leaves the bank. Every way allowed by `cap` is
    /// worked out and the one that disturbs least wins (see `Shuffle`):
    /// fewest other slots moved, then the shortest total distance they move,
    /// then the shortest trip for `g` itself (so it crawls toward the target
    /// and stops at the first good spot). Ties go to the gentler level.
    pub fn plan_reorder(&self, g: usize, at: usize, cap: Shuffle) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let spans = self.group_spans();
        let span = spans[g].clone();
        let len = span.len();
        let bk = self.bank;
        let bank = span.start / bk;
        let base = (bank * bk).max(self.first);
        let end = self.bank_end(span.start);
        let mut others: Vec<usize> = (0..spans.len()).filter(|&h| h != g && spans[h].start / bk == bank).collect();
        others.sort_by_key(|&h| spans[h].start);
        let at = at.min(others.len());
        let a = at.checked_sub(1).map(|i| others[i]);
        let b = others.get(at).copied();
        let name = |h: usize| format!("\"{}\"", self.groups[h].name);
        let place = match (a, b) {
            (Some(a), Some(b)) => format!("between {} and {}", name(a), name(b)),
            (Some(a), None) => format!("after {}", name(a)),
            (None, Some(b)) => format!("before {}", name(b)),
            (None, None) => return Err(format!("\"{}\" is the only group in its bank", gr.name)),
        };
        if others.iter().filter(|&&h| spans[h].start < span.start).count() == at {
            return Err(format!("\"{}\" is already {place}", gr.name));
        }
        let lo = a.map_or(base, |a| spans[a].end);
        let hi = b.map_or(end, |b| spans[b].start);
        let before = positions(self.order);
        let mine: HashSet<u16> = gr.members.iter().copied().collect();
        // The spacing the user keeps between groups in this bank: the
        // smallest gap they left between two neighbouring groups (1 if they
        // all touch). The moved group tries to keep that much on each side.
        let bank_spans = |groups: &[Group], pos: &[usize]| -> Vec<Range<usize>> {
            let mut v: Vec<Range<usize>> = groups
                .iter()
                .map(|h| span_of(h, pos))
                .filter(|s| !s.is_empty() && s.start / bk == bank)
                .collect();
            v.sort_by_key(|s| s.start);
            v
        };
        let pad = {
            let now = bank_spans(self.groups, &before);
            now.windows(2).map(|w| w[1].start.saturating_sub(w[0].end)).filter(|&d| d > 0).min().unwrap_or(1)
        };
        // Is the result in the wanted order, inside the bank, and how much
        // did it disturb? `None`: not usable. Scored, lowest wins: groups
        // left touching (gaps between groups are only given up when there's
        // no room to keep them), slots moved, their distance, how far the
        // moved group falls short of `pad` on its sides, its own trip.
        let grouped: HashSet<u16> = self.groups.iter().flat_map(|g| g.members.iter().copied()).collect();
        let judge = |p: &Plan| -> Option<(usize, usize, usize, usize, usize)> {
            if p.groups.len() != self.groups.len() {
                return None;
            }
            if p.order.iter().zip(self.order).enumerate().any(|(i, (x, y))| x != y && i / bk != bank) {
                return None;
            }
            let pos = positions(&p.order);
            let new = |h: usize| span_of(&p.groups[h], &pos);
            let gs = new(g);
            if a.is_some_and(|a| new(a).end > gs.start) || b.is_some_and(|b| new(b).start < gs.end) {
                return None;
            }
            let (mut moved, mut dist) = (0, 0);
            for (i, &id) in p.order.iter().enumerate() {
                let was = before[id as usize];
                if was != i && !mine.contains(&id) && (!self.free(id) || grouped.contains(&id)) {
                    moved += 1;
                    dist += was.abs_diff(i);
                }
            }
            let touching = bank_spans(&p.groups, &pos).windows(2).filter(|w| w[1].start <= w[0].end).count();
            let short = a.map_or(0, |a| pad.saturating_sub(gs.start.saturating_sub(new(a).end)))
                + b.map_or(0, |b| pad.saturating_sub(new(b).start.saturating_sub(gs.end)));
            Some((touching, moved, dist, short, gs.start.abs_diff(span.start)))
        };
        type Score = (usize, usize, usize, usize, usize);
        let mut best: Option<(Score, Shuffle, Plan)> = None;
        let mut offer = |score: Score, level: Shuffle, p: Plan| {
            if level <= cap && best.as_ref().is_none_or(|(s, l, _)| (score, level) < (*s, *l)) {
                best = Some((score, level, p));
            }
        };
        // Fit / Push: lift the group out and drop it on each spot between
        // its new neighbours, pushing aside as a group drop does.
        let last = end.saturating_sub(len);
        for top in lo.min(last)..=hi.min(last) {
            let Ok(p) = self.plan_picked(SlotDrop::Group { g, top }, None) else { continue };
            let Some(score) = judge(&p) else { continue };
            let level = if score.1 == 0 { Shuffle::Fit } else { Shuffle::Push };
            offer(score, level, p);
        }
        // Rotate: the slots between the group and its new neighbour slide
        // over by its length, gaps and all; the group takes their place.
        // Always fits, since the bank's slot count doesn't change.
        if cap >= Shuffle::Rotate {
            let mut order = self.order.to_vec();
            if span.end <= lo {
                order[span.start..lo].rotate_left(len);
            } else {
                order[hi..span.end].rotate_right(len);
            }
            let p = Plan { order, groups: self.groups.to_vec(), note: String::new() };
            if self.check_groups(&p.order, &p.groups).is_ok() {
                if let Some(score) = judge(&p) {
                    offer(score, Shuffle::Rotate, p);
                }
            }
        }
        let Some(((_, moved, _, _, _), level, p)) = best else {
            return Err(match cap {
                Shuffle::Fit => format!("No free room {place} for \"{}\" ({len} slots); release ⇧ to push slots aside", gr.name),
                _ => format!("Can't move \"{}\" {place} without leaving its bank", gr.name),
            });
        };
        let land = span_of(&p.groups[g], &positions(&p.order)).start;
        let how = match level {
            Shuffle::Fit => "fits in free slots, nothing else moves".to_string(),
            Shuffle::Push => format!("{moved} slot(s) pushed into free room"),
            Shuffle::Rotate => format!("no free room: {moved} slot(s) slide over by {len}"),
        };
        let note = format!("Move \"{}\" {place}, to {} · {how}", gr.name, self.range(land, len));
        Ok(Plan { note, ..p })
    }

    /// Arrow keys on a selected group: slides it `step` slots (negative: up)
    /// inside its bank; the slots it passes over, empty or not, go to its
    /// other side. |step| > 1 means "up to": it stops short of a neighbour
    /// group or the bank's edge. A one-slot step into a neighbour group
    /// hops the whole neighbour. At the bank's edge it crosses into the next
    /// bank (`plan_cross`).
    pub fn plan_step(&self, g: usize, step: i32) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let spans = self.group_spans();
        let span = spans[g].clone();
        let len = span.len();
        let base = (span.start / self.bank * self.bank).max(self.first);
        let end = self.bank_end(span.start);
        let down = step > 0;
        let room = if down { end - span.end } else { span.start - base };
        if room == 0 {
            return self.plan_cross(g, down);
        }
        let want = (step.unsigned_abs() as usize).min(room);
        // The run slid over must not end inside another group.
        let edge = |k: usize| if down { span.end + k } else { span.start - k };
        let cut = spans.iter().enumerate().find(|&(h, r)| h != g && r.start < edge(want) && edge(want) < r.end).map(|(_, r)| r.clone());
        let k = match cut {
            None => want,
            Some(r) => {
                let (shrink, grow) = if down {
                    (r.start - span.end, r.end - span.end)
                } else {
                    (span.start - r.end, span.start - r.start)
                };
                if shrink > 0 && step.abs() > 1 { shrink } else { grow }
            }
        };
        let mut order = self.order.to_vec();
        if down {
            order[span.start..span.end + k].rotate_right(k);
        } else {
            order[span.start - k..span.end].rotate_left(k);
        }
        self.check_groups(&order, self.groups)?;
        let land = if down { span.start + k } else { span.start - k };
        let passed = (if down { span.end..span.end + k } else { span.start - k..span.start })
            .filter(|&s| !self.free(self.order[s]))
            .count();
        let mut note = format!("Move \"{}\" to {}", gr.name, self.range(land, len));
        if passed > 0 {
            note += &format!(" · {passed} slot(s) moved {}", if down { "up" } else { "down" });
        }
        Ok(Plan { order, groups: self.groups.to_vec(), note })
    }

    /// `plan_step` past the bank's edge: group `g` lands at the near end of
    /// the next bank (top when going down, bottom when going up), that
    /// bank's slots shifting away into its free slots nearest the edge
    /// (loose empties, not group members). The empties it uses fill the
    /// group's old place. Refused when the bank lacks room or already has
    /// a group of that name.
    fn plan_cross(&self, g: usize, down: bool) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let span = self.group_spans()[g].clone();
        let len = span.len();
        let n = self.order.len();
        let bk = self.bank;
        let bank = span.start / bk;
        let tb = match (down, bank) {
            (false, 0) => return Err(format!("\"{}\" is at the top of bank {}", gr.name, self.bname(0))),
            (false, b) => b - 1,
            (true, b) => b + 1,
        };
        let (tbase, tend) = ((tb * bk).max(self.first), ((tb + 1) * bk).min(n));
        if tbase >= n {
            return Err(format!("\"{}\" is at the end of the last bank", gr.name));
        }
        if let Some(why) = self.name_taken(&gr.name, tb, Some(g)) {
            return Err(why);
        }
        let grouped: HashSet<u16> = self.groups.iter().flat_map(|g| g.members.iter().copied()).collect();
        let mut cells: Vec<u16> = self.order[tbase..tend].to_vec();
        let mut loose: Vec<usize> = (0..cells.len()).filter(|&i| self.free(cells[i]) && !grouped.contains(&cells[i])).collect();
        if loose.len() < len {
            return Err(format!("Bank {} has {} free slot(s); \"{}\" needs {len}", self.bname(tb), loose.len(), gr.name));
        }
        if down {
            loose.truncate(len);
        } else {
            loose.drain(..loose.len() - len);
        }
        let pushed = if down { loose[len - 1] + 1 - len } else { cells.len() - loose[0] - len };
        let empties: Vec<u16> = loose.iter().map(|&i| cells[i]).collect();
        for &i in loose.iter().rev() {
            cells.remove(i);
        }
        let members = self.order[span.clone()].to_vec();
        let cells = if down { [members, cells].concat() } else { [cells, members].concat() };
        let mut order = self.order.to_vec();
        order[tbase..tend].copy_from_slice(&cells);
        order[span.clone()].copy_from_slice(&empties);
        self.check_groups(&order, self.groups)?;
        let land = if down { tbase } else { tend - len };
        let mut note = format!("Move \"{}\" to bank {}, {}", gr.name, self.bname(tb), self.range(land, len));
        let shifted = (if down { tbase + len..tbase + len + pushed } else { tend - len - pushed..tend - len })
            .filter(|&s| !self.free(order[s]))
            .count();
        if shifted > 0 {
            note += &format!(" · {shifted} slot(s) shifted {}", if down { "down" } else { "up" });
        }
        Ok(Plan { order, groups: self.groups.to_vec(), note })
    }

    /// `pick`: the slots to move instead of the selection.
    fn plan_picked(&self, d: SlotDrop, pick: Option<&[usize]>) -> Result<Plan, String> {
        let n = self.order.len();
        let first = self.first;
        let spans = self.group_spans();
        let (picked, whole): (Vec<usize>, Option<usize>) = match d {
            SlotDrop::Group { g, .. } => (spans.get(g).ok_or("No such group")?.clone().collect(), Some(g)),
            _ => {
                let from: Vec<usize> = match pick {
                    Some(p) => p.to_vec(),
                    None => self.sel.iter().copied().collect(),
                };
                (from.into_iter().filter(|&s| s >= first && s < n).collect(), None)
            }
        };
        if picked.is_empty() {
            return Err("Nothing to move".into());
        }
        let m = picked.len();
        if let SlotDrop::Place(top) = d {
            return self.plan_place(top, &picked, &spans);
        }
        let moved: Vec<u16> = picked.iter().map(|&s| self.order[s]).collect();
        let mut groups = self.groups.to_vec();
        let left: Vec<usize> = match whole {
            Some(_) => Vec::new(),
            None => groups.iter().enumerate().filter(|(_, g)| g.members.iter().any(|x| moved.contains(x))).map(|(i, _)| i).collect(),
        };
        // Lift the moved slots out. A group they leave closes up, pulling
        // what follows up as far as the next empty slot (in its bank).
        let mut cells: Vec<Option<u16>> = self.order.iter().copied().map(Some).collect();
        let mut closes = Vec::new();
        for &p in &picked {
            if whole.is_none() && left.iter().any(|&g| groups[g].members.contains(&self.order[p])) {
                closes.push(p);
            }
            cells[p] = None;
        }
        // Containers keep their size after losing members, if there's room.
        let sizes: Vec<Option<usize>> = groups
            .iter()
            .map(|g| g.members.iter().any(|&x| self.free(x)).then_some(g.members.len()))
            .collect();
        if whole.is_none() {
            for g in &mut groups {
                g.members.retain(|x| !moved.contains(x));
            }
        }
        let grouped: HashSet<u16> = groups.iter().flat_map(|g| g.members.iter().copied()).collect();
        // Free for a push: empty and in no group; dropping into a group may
        // also fill that group's own empty slots. A page that inserts lets
        // only the slots just left take up the shift.
        let into = match d {
            SlotDrop::Into { g, .. } => Some(g),
            _ => None,
        };
        let loose = |id: u16| self.free(id) && !grouped.contains(&id);
        // Only a group being dropped into takes up the push with its own
        // empty slots. Loose empty slots are information: they travel with
        // the push like samples do.
        let absorbs = |id: u16| self.free(id) && into.is_some_and(|g| groups[g].members.contains(&id));
        // A group left closes up: the rest of its bank moves up, gaps
        // included, and the lifted slot goes to the bank's end.
        for &p in closes.iter().rev() {
            let end = self.bank_end(p);
            cells[p..end].rotate_left(1);
        }
        // Where the moved slots go, in the lifted-out layout.
        let at = match d {
            SlotDrop::Shift(top) | SlotDrop::Group { top, .. } => top.clamp(first, n - m),
            SlotDrop::Into { g, before, .. } => {
                let find = |id: u16| cells.iter().position(|c| *c == Some(id));
                match before {
                    Some(id) => find(id).ok_or("The slot to insert before moved")?,
                    None => match groups[g].members.iter().filter_map(|&x| find(x)).max() {
                        Some(last) => last + 1,
                        None => return Err("Drag the group by its name or frame to move all of it".into()),
                    },
                }
            }
            SlotDrop::Place(_) | SlotDrop::Split { .. } => unreachable!(),
        };
        // Put them in, pushing what's there down until enough empty slots
        // (including the ones just left) take up the shift. When there's no
        // room below (bank or list end, or a group would be split), push
        // what's above the landing spot up instead.
        let place = |up: bool| -> Result<(Vec<u16>, Vec<Group>, usize), String> {
            let mut cells = cells.clone();
            let mut groups = groups.clone();
            let mut spare = Vec::new();
            let mut carry = std::collections::VecDeque::new();
            // Slots pushed into, in push order (away from the landing spot).
            let mut path = Vec::new();
            let no_room = if up { "No empty slot above to make room" } else { "No empty slot below to make room" };
            // The push runs until a lifted slot takes it up or the bank's
            // edge; whatever is still carried there needs that many loose
            // empty slots dropped, the ones furthest from the landing first.
            let mut incoming: Box<dyn Iterator<Item = u16>> =
                if up { Box::new(moved.iter().rev().copied()) } else { Box::new(moved.iter().copied()) };
            let (start, edge) = if up {
                let end = if into.is_some() { at } else { at + m };
                let lo = end.checked_sub(m).ok_or(no_room)?;
                (end, (lo / self.bank * self.bank).max(first))
            } else {
                (at, self.bank_end(at + m - 1))
            };
            let mut i = start;
            let mut left = Vec::new();
            loop {
                let Some(next) = incoming.next().or_else(|| carry.pop_front()) else { break };
                let out = if up { i <= edge } else { i >= edge };
                if out {
                    left.push(next);
                    if incoming.next().is_some() {
                        return Err(no_room.into());
                    }
                    left.extend(carry.drain(..));
                    break;
                }
                if up {
                    i -= 1;
                }
                path.push(i);
                match cells[i].replace(next) {
                    Some(old) if absorbs(old) => spare.push(old),
                    Some(old) => carry.push_back(old),
                    None => {}
                }
                if !up {
                    i += 1;
                }
            }
            if !left.is_empty() {
                let mut seq: Vec<u16> = path.iter().map(|&p| cells[p].expect("pushed slots are filled")).collect();
                seq.extend(&left);
                let mut need = left.len();
                for k in (0..seq.len()).rev() {
                    if need == 0 {
                        break;
                    }
                    if loose(seq[k]) && !moved.contains(&seq[k]) {
                        spare.push(seq.remove(k));
                        need -= 1;
                    }
                }
                if need > 0 {
                    return Err(no_room.into());
                }
                for (&p, id) in path.iter().zip(seq) {
                    cells[p] = Some(id);
                }
            }
            let spare_ids: HashSet<u16> = spare.iter().copied().collect();
            let mut spare = spare.into_iter();
            for c in cells.iter_mut().filter(|c| c.is_none()) {
                *c = spare.next();
            }
            let order: Vec<u16> = cells.into_iter().map(|c| c.expect("every lifted slot refilled")).collect();
            let mut filled = 0;
            if let Some(g) = into {
                let before = groups[g].members.len();
                groups[g].members.retain(|x| !spare_ids.contains(x));
                filled = before - groups[g].members.len();
                groups[g].members.extend(&moved);
            }
            let pos = positions(&order);
            for g in &mut groups {
                g.members.sort_by_key(|x| pos[*x as usize]);
            }
            self.regrow(&mut groups, &sizes, &order, &pos);
            self.check_groups(&order, &groups.iter().filter(|g| !g.members.is_empty()).cloned().collect::<Vec<_>>())?;
            Ok((order, groups, filled))
        };
        let (upward, (order, mut groups, filled)) = match place(false) {
            // Into a container: filling its empty slots above beats growing.
            Ok(r) if into.is_some() && r.2 < m => match place(true) {
                Ok(u) if u.2 > r.2 => (true, u),
                _ => (false, r),
            },
            Ok(r) => (false, r),
            Err(below) => match place(true) {
                Ok(r) => (true, r),
                Err(_) => return Err(below),
            },
        };
        let pos = positions(&order);
        let emptied: Vec<String> = groups.iter().filter(|g| g.members.is_empty()).map(|g| g.name.clone()).collect();
        groups.retain(|g| !g.members.is_empty());

        // What it says.
        let before = positions(self.order);
        let shifted: Vec<(usize, usize)> = (first..n)
            .map(|p| order[p])
            .filter(|&id| !moved.contains(&id) && !self.free(id))
            .map(|id| (before[id as usize], pos[id as usize]))
            .filter(|(a, b)| a != b)
            .collect();
        let down: Vec<usize> = shifted.iter().filter(|(a, b)| b > a).map(|&(_, b)| b).collect();
        let ups: Vec<usize> = shifted.iter().filter(|(a, b)| b < a).map(|&(_, b)| b).collect();
        let up = ups.len();
        let land = pos[moved[0] as usize];
        let mut note = match d {
            SlotDrop::Group { g, .. } => format!("Move group \"{}\" to {}", self.groups[g].name, self.range(land, m)),
            SlotDrop::Into { g, .. } => {
                let name = &self.groups[g].name;
                if moved.iter().all(|x| self.groups[g].members.contains(x)) {
                    format!("Reorder in \"{name}\" at {}", self.range(land, m))
                } else if filled > 0 {
                    format!("Fill {filled} empty slot(s) of \"{name}\" at {}", self.range(land, m))
                } else {
                    format!("Add to \"{name}\" at {}", self.range(land, m))
                }
            }
            _ => {
                if picked.iter().copied().eq(land..land + m) && shifted.is_empty() {
                    return Err(format!("Already at {}", self.range(land, m)));
                }
                format!("Move to {}", self.range(land, m))
            }
        };
        if let Some(&last) = down.iter().max() {
            note += &format!(" · {} pushed down to {}", down.len(), self.label(last));
        }
        if upward {
            if let Some(&top) = ups.iter().min() {
                note += &format!(" · no room below: {up} pushed up to {}", self.label(top));
            }
        } else if up > 0 {
            note += &format!(" · {up} pulled up");
        }
        if whole.is_none() && !matches!(d, SlotDrop::Into { .. }) {
            for &g in &left {
                note += &format!(" · leaves \"{}\"", self.groups[g].name);
            }
        }
        for name in emptied {
            note += &format!(" · \"{name}\" ends (empty)");
        }
        Ok(Plan { order, groups, note })
    }

    /// ⌥: swap the selection into `top…` in place. Groups don't take part.
    fn plan_place(&self, top: usize, picked: &[usize], spans: &[Range<usize>]) -> Result<Plan, String> {
        let n = self.order.len();
        let m = picked.len();
        let top = top.clamp(self.first, n - m);
        let hit = |s: usize| spans.iter().any(|r| r.contains(&s));
        if picked.iter().any(|&s| hit(s)) || (top..top + m).any(hit) {
            return Err("⌥ swap leaves groups alone: release ⌥ to move into or out of a group".into());
        }
        let mut order = self.order.to_vec();
        slot_list::place(&mut order, picked, top, self.first);
        let range = self.range(top, m);
        if picked.iter().copied().eq(top..top + m) {
            return Err(format!("Already at {range}"));
        }
        let swaps = (top..top + m).filter(|t| !picked.contains(t) && !self.free(self.order[*t])).count();
        let note = if swaps > 0 {
            format!("Swap into {range} · {swaps} go to the freed slots")
        } else {
            format!("Place at {range}")
        };
        Ok(Plan { order, groups: self.groups.to_vec(), note })
    }

    /// Empty and in no group: a slot a push or pull may use up. A group's
    /// empty slots move with it.
    fn loose(&self, id: u16) -> bool {
        self.free(id) && !self.groups.iter().any(|g| g.members.contains(&id))
    }

    /// `n` empty slots at `at`: it and everything below it in the bank
    /// move down together, gaps included (empty slots are information too).
    /// Each new empty takes the bank's last loose empty slot, so trailing
    /// empties go first. Groups in the way move down whole.
    pub fn plan_insert_empty(&self, at: usize, n: usize) -> Result<Plan, String> {
        if let Some(g) = self.group_at(at).filter(|&g| self.group_spans()[g].start != at) {
            return Err(format!("Groups stay packed: insert above or below \"{}\"", self.groups[g].name));
        }
        let end = self.bank_end(at);
        let mut order = self.order.to_vec();
        let mut made = 0;
        let mut inner = Vec::new();
        while made < n {
            let from = at + made.max(1);
            let Some(e) = (from..end).rev().find(|&i| self.loose(order[i])) else { break };
            // A gap with something after it is used up only when the bank
            // has no empty slots left at its end.
            if (e + 1..end).any(|i| !self.free(order[i])) {
                inner.push(e);
            }
            order[at..=e].rotate_right(1);
            made += 1;
        }
        if made == 0 {
            return Err("No empty slot left in this bank (outside groups) to make room".into());
        }
        self.check_groups(&order, self.groups)?;
        let short = if made < n { format!(" (only {made} of {n}: no more empty slots in this bank)") } else { String::new() };
        let mut note = format!("Inserted {made} empty at {}; the rest of the bank moves down{short}", self.range(at, made));
        if !inner.is_empty() {
            note += &format!(" · no room at the bank's end: used up {} gap(s) further down", inner.len());
        }
        Ok(Plan { order, groups: self.groups.to_vec(), note })
    }

    /// Where an empty slot would go right above (`below` false) or below
    /// group `g`, when the neighbour on that side, in its bank, holds
    /// something. `Err` says why not.
    pub fn gap_beside(&self, g: usize, below: bool) -> Result<usize, String> {
        let span = self.group_spans().get(g).cloned().ok_or("No such group")?;
        let (at, nb) = if below { (span.end, span.end) } else { (span.start, span.start.wrapping_sub(1)) };
        if nb < self.first || nb >= self.order.len() || nb / self.bank != span.start / self.bank {
            return Err(format!("The group is at the {} of its bank", if below { "end" } else { "start" }));
        }
        if self.free(self.order[nb]) {
            return Err(format!("{} is already empty", self.label(nb)));
        }
        Ok(at)
    }

    /// An empty slot right outside group `g` (`gap_beside`), moving the rest
    /// of the bank down (`plan_insert_empty`).
    pub fn plan_gap_beside(&self, g: usize, below: bool) -> Result<Plan, String> {
        let at = self.gap_beside(g, below)?;
        self.plan_insert_empty(at, 1)
    }

    /// Takes out the empty slots at `ats`: everything below each in its bank
    /// moves up together, gaps included, and the freed slot goes to the
    /// bank's end. Groups in the way move up whole; a grouped empty slot
    /// leaves its group.
    pub fn plan_close_gaps(&self, ats: &[usize]) -> Result<Plan, String> {
        let mut ats: Vec<usize> = ats.iter().copied().filter(|&a| a >= self.first && a < self.order.len()).collect();
        ats.sort_unstable();
        ats.dedup();
        let mut order = self.order.to_vec();
        let mut groups = self.groups.to_vec();
        // Bottom-up, so closing one never moves a gap still to close.
        for &at in ats.iter().rev() {
            let id = order[at];
            if !self.free(id) {
                continue;
            }
            for g in &mut groups {
                g.members.retain(|&x| x != id);
            }
            let end = self.bank_end(at);
            order[at..end].rotate_left(1);
        }
        let emptied: Vec<String> = groups.iter().filter(|g| g.members.is_empty()).map(|g| g.name.clone()).collect();
        groups.retain(|g| !g.members.is_empty());
        self.check_groups(&order, &groups)?;
        let mut note = match ats.as_slice() {
            [a] => format!("Closed gap at {}", self.label(*a)),
            _ => format!("Closed {} gaps", ats.len()),
        };
        for name in emptied {
            note += &format!(" · \"{name}\" ends (empty)");
        }
        Ok(Plan { order, groups, note })
    }

    /// Every group packed, inside one bank.
    fn check_groups(&self, order: &[u16], groups: &[Group]) -> Result<(), String> {
        let pos = positions(order);
        for g in groups {
            let span = span_of(g, &pos);
            if span.len() != g.members.len() {
                return Err(format!("That would split the group \"{}\"", g.name));
            }
            if span.start < self.first || span.start / self.bank != (span.end - 1) / self.bank {
                return Err(format!("That would push the group \"{}\" past the end of its bank", g.name));
            }
        }
        Ok(())
    }

    /// After an edit that doesn't know about groups: each keeps its longest
    /// packed run in one bank; members outside it leave.
    pub fn repair_groups(&self, order: &[u16]) -> Vec<Group> {
        let pos = positions(order);
        self.groups
            .iter()
            .filter_map(|g| {
                let mut ps: Vec<usize> = g.members.iter().map(|&x| pos[x as usize]).collect();
                ps.sort_unstable();
                let mut best: Range<usize> = 0..0;
                let mut i = 0;
                while i < ps.len() {
                    let mut j = i + 1;
                    while j < ps.len() && ps[j] == ps[j - 1] + 1 && ps[j] / self.bank == ps[i] / self.bank {
                        j += 1;
                    }
                    if j - i > best.len() {
                        best = i..j;
                    }
                    i = j;
                }
                let members: Vec<u16> = ps[best].iter().filter(|&&p| p >= self.first).map(|&p| order[p]).collect();
                (!members.is_empty()).then(|| Group { name: g.name.clone(), members })
            })
            .collect()
    }

    /// Slot number (within its bank) in the middle of a group's span.
    fn mid_of(&self, span: &Range<usize>) -> usize {
        (span.start + (span.len().max(1) - 1) / 2) % self.bank
    }

    /// Containers (`sizes[g]` set: they held an empty slot) that lost members
    /// take free, ungrouped slots right after them, then right before, in
    /// their bank, back up to their old size.
    fn regrow(&self, groups: &mut [Group], sizes: &[Option<usize>], order: &[u16], pos: &[usize]) {
        let n = order.len();
        for g in 0..groups.len() {
            let Some(size) = sizes.get(g).copied().flatten() else { continue };
            if groups[g].members.is_empty() {
                continue;
            }
            while groups[g].members.len() < size {
                let span = span_of(&groups[g], pos);
                let bank = span.start / self.bank;
                let open = |p: usize| {
                    p >= self.first
                        && p < n
                        && p / self.bank == bank
                        && self.free(order[p])
                        && !groups.iter().any(|h| h.members.contains(&order[p]))
                };
                if open(span.end) {
                    groups[g].members.push(order[span.end]);
                } else if span.start > 0 && open(span.start - 1) {
                    groups[g].members.insert(0, order[span.start - 1]);
                } else {
                    break;
                }
            }
        }
    }
}

pub fn span_of(g: &Group, pos: &[usize]) -> Range<usize> {
    let ps = g.members.iter().map(|&x| pos[x as usize]);
    let a = ps.clone().min().unwrap_or(0);
    let b = ps.max().map_or(0, |b| b + 1);
    a..b
}

/// A group control was used (frame, tab, jump row, menu). The page applies
/// it through [`GroupHost::group_act`].
pub enum GroupAct {
    /// A group grabbed by its name, frame or jump button: `(group, slot
    /// grabbed, how far down that row)`.
    DragStart(usize, usize, f32),
    /// Make a group of these slots (asks for its name).
    New(Vec<usize>),
    Rename(usize),
    Dissolve(usize),
    /// Drop the group's empty slots; what it holds closes up.
    Compact(usize),
    /// Insert an empty slot right outside the group: `(group, below)`.
    /// Above pushes the group down; below pushes what follows it.
    Gap(usize, bool),
    /// Hue offset for every group of this name (0 forgets it).
    Hue(String, f32),
    Select(usize),
    /// Line the other pane up on its group of the same name.
    Align(usize),
    /// Make an empty group like this one in every bank without one.
    Spread(usize),
    /// Move this group toward the mean slot of its same-named groups elsewhere.
    Gather(usize),
    /// Move these slots into group `g` at its end ("Move to group").
    Join(usize, Vec<usize>),
    /// Forget every group that holds nothing.
    RemoveEmpty,
    /// Group `g` to place `at` among its bank's groups (jump row drop).
    Reorder(usize, usize, Shuffle),
    /// Jump to the group (jump row button).
    GoTo(usize),
}

impl GroupAct {
    /// Changes slots or groups (refused while the page can't take edits).
    pub fn edits(&self) -> bool {
        !matches!(self, Self::DragStart(..) | Self::New(_) | Self::Rename(_) | Self::Select(_) | Self::Align(_) | Self::GoTo(_) | Self::Hue(..))
    }
}

/// A page that has groups: how it applies a plan and moves its view.
pub trait GroupHost {
    fn slots(&self) -> Slots<'_>;
    /// Takes the plan's order and groups as one undoable edit and shows its
    /// note. Then selects group `select` (an index in `p.groups`) and, with
    /// `lead` (`Some(down)`), puts the cursor on its leading edge and in view.
    fn commit_plan(&mut self, p: Plan, select: Option<usize>, lead: Option<bool>);
    fn note(&mut self, msg: String);
    fn select_group(&mut self, g: usize);
    fn go_to_group(&mut self, g: usize, side: usize);
    fn align_group(&mut self, g: usize, side: usize);
    fn group_drag_start(&mut self, g: usize, slot: usize, frac: f32, side: usize);
    fn end_group_drag(&mut self);
    fn ask_group_name(&mut self, g: Option<usize>, slots: Vec<usize>, name: String);
    fn set_hue(&mut self, key: String, h: f32);
    fn set_active(&mut self, side: usize);

    fn apply(&mut self, r: Result<Plan, String>, select: Option<usize>) {
        match r {
            Ok(p) => self.commit_plan(p, select, None),
            Err(why) => self.note(why),
        }
    }

    fn apply_drop(&mut self, d: SlotDrop) {
        let s = self.slots();
        let r = s.plan(d).map(|p| {
            let held = s.held_after(d, &p);
            (p, held)
        });
        drop(s);
        match r {
            Ok((p, held)) => self.commit_plan(p, held, None),
            Err(why) => self.note(why),
        }
    }

    /// Arrow keys on a held group: `plan_step`, keeping it selected and in view.
    fn step_group(&mut self, g: usize, step: i32) {
        let r = self.slots().plan_step(g, step);
        match r {
            Ok(p) => self.commit_plan(p, Some(g), Some(step > 0)),
            Err(why) => self.note(why),
        }
    }

    fn new_group(&mut self, pick: &[usize], name: String) {
        let r = self.slots().new_group(pick, name);
        self.apply(r, None);
    }

    fn rename_group(&mut self, g: usize, name: String) {
        let r = self.slots().rename_group(g, name);
        self.apply(r, None);
    }

    fn group_act(&mut self, a: GroupAct, side: usize) {
        match a {
            GroupAct::DragStart(g, s, frac) => self.group_drag_start(g, s, frac, side),
            GroupAct::New(slots) => self.ask_group_name(None, slots, String::new()),
            GroupAct::Rename(g) => {
                let name = self.slots().groups.get(g).map(|gr| gr.name.clone());
                if let Some(name) = name {
                    self.ask_group_name(Some(g), Vec::new(), name);
                }
            }
            GroupAct::Dissolve(g) => {
                let r = self.slots().dissolve_group(g);
                self.apply(r, None);
            }
            GroupAct::Compact(g) => {
                let r = self.slots().compact_group(g);
                self.apply(r, None);
            }
            GroupAct::Gap(g, below) => {
                let r = self.slots().plan_gap_beside(g, below);
                self.apply(r, None);
            }
            GroupAct::Hue(key, h) => self.set_hue(key, h),
            GroupAct::Select(g) => {
                self.set_active(side);
                self.select_group(g);
            }
            GroupAct::Align(g) => self.align_group(g, side),
            GroupAct::Spread(g) => {
                let r = self.slots().spread_group(g);
                self.apply(r, None);
            }
            GroupAct::Gather(g) => {
                let r = self.slots().plan_gather(g);
                self.apply(r, None);
            }
            GroupAct::Join(g, pick) => {
                self.set_active(side);
                let r = self.slots().plan_join_in_bank(g, &pick);
                self.apply(r, None);
            }
            GroupAct::RemoveEmpty => {
                let r = self.slots().remove_empty_groups();
                self.apply(r, None);
            }
            GroupAct::Reorder(g, at, cap) => {
                self.set_active(side);
                let r = self.slots().plan_reorder(g, at, cap);
                self.apply(r, Some(g));
                self.end_group_drag();
            }
            GroupAct::GoTo(g) => self.go_to_group(g, side),
        }
    }
}

/// Naming a group: `(group to rename, or None for a new one from these
/// slots, name typed so far, focus the field next frame)`.
pub type Naming = (Option<usize>, Vec<usize>, String, bool);

/// The name dialog. `Some(true)` to go ahead, `Some(false)` to cancel.
/// `kept` says where groups are kept, under a new group's slot count.
pub fn name_dialog(ctx: &egui::Context, r: &mut Naming, kept: &str) -> Option<bool> {
    let (g, slots, name, focus) = r;
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("slot_group_name_modal")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.heading(if g.is_some() { "Rename group" } else { "New group" });
        ui.add_space(6.0);
        let te = ui.add(egui::TextEdit::singleline(name).hint_text("e.g. Tuned to A").desired_width(f32::INFINITY));
        if std::mem::take(focus) {
            te.request_focus();
        }
        if g.is_none() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("{} slot(s). {kept}", slots.len())).weak());
        }
        ui.add_space(8.0);
        let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            let ok = !name.trim().is_empty();
            if ui.add_enabled(ok, egui::Button::new(if g.is_some() { "Rename" } else { "Make group" })).clicked() || (ok && enter) {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(false);
            }
        });
    });
    answer
}

/// Runs a finished name dialog on `host`.
pub fn finish_naming(ctx: &egui::Context, naming: &mut Option<Naming>, kept: &str, host: &mut impl GroupHost) {
    let Some(r) = naming.as_mut() else { return };
    let Some(ok) = name_dialog(ctx, r, kept) else { return };
    let (g, slots, name, _) = naming.take().unwrap_or_default();
    let name = name.trim().to_owned();
    if ok {
        match g {
            Some(g) => host.rename_group(g, name),
            None => host.new_group(&slots, name),
        }
    }
}
