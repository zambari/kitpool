//! Several Elektron units at once. Each has its own device worker (see
//! [`crate::device::Device`]); the app shows one, the *active* unit, and
//! keeps the others' share of its state parked in a [`UnitCtx`]: project,
//! page, pool edits, +Drive folders. Switching trades the two, so each unit
//! comes back where it was left, and a job left running on the other one
//! keeps going. Its events are applied to its parked state
//! ([`App::absorb_background`]). Copies (patterns, tracks, a sound) follow
//! the user to the newly active unit, like into another project.

use super::*;
use crate::ui::elek_view::{DevicePick, ElekState};

/// Declares [`UnitCtx`] and [`App::swap_unit`] from one list, so a field
/// added to a unit's share can't be forgotten in either.
macro_rules! unit_state {
    ($($field:ident: $ty:ty),* $(,)?) => {
        /// A unit's share of the app, parked while another unit is active.
        /// Each field is the [`App`] field of the same name.
        #[derive(Default)]
        pub(super) struct UnitCtx {
            $($field: $ty,)*
        }

        impl App {
            /// Trade the active unit's fields with `ctx`'s.
            fn swap_unit(&mut self, ctx: &mut UnitCtx) {
                $(std::mem::swap(&mut self.$field, &mut ctx.$field);)*
            }
        }
    };
}

unit_state! {
    device_status: Option<Result<String, String>>,
    device_last: Option<DevicePick>,
    elek: ElekState,
    elek_greeted: bool,
    elek_preloaded: bool,
    first_dock: bool,
    reconnect_ask: Option<usize>,
    pool_fetch: elek::PoolFetch,
    update_push: std::collections::HashSet<u16>,
    pool_rename: Option<(u64, (u32, u32), String)>,
    relink_after: Option<(u64, (u32, u32), PathBuf)>,
    changed_paths: std::collections::HashSet<PathBuf>,
    changed_at: Option<std::time::Instant>,
    seen_slot: Option<(u32, u32)>,
    pool_edit: Option<pool_edit::PoolEdit>,
    slot_save: Option<pool_edit::SlotSave>,
    staged_upload: Option<StagedUpload>,
    device_origin: Option<(PathBuf, PathBuf)>,
    drive_dirs: std::collections::HashMap<u64, PathBuf>,
    unit_lost: bool,
}

impl App {
    /// Follow the device layer's active unit (picked by the user, or the
    /// first one present after the active one was unplugged): park the
    /// unit left and bring back the new one's state. Call each frame
    /// before absorbing events.
    pub(super) fn sync_unit(&mut self) {
        let Some(unit) = device::active_unit() else { return };
        if self.unit_name.as_ref() == Some(&unit) {
            return;
        }
        let Some(left) = self.unit_name.replace(unit.clone()) else {
            // The first unit this run: the state there is is its own.
            return;
        };
        crate::log::line(format!("units: {left} parked, {unit} shown"));
        let fresh = !self.parked_units.contains_key(&unit);
        let mut ctx = self.parked_units.remove(&unit).unwrap_or_else(|| self.fresh_unit_ctx());
        self.drive_dirs = self.device_tab_dirs();
        self.swap_unit(&mut ctx);
        let p = self.elek.project.as_deref();
        self.elek.proj.carry_copies(&mut ctx.elek.proj, p);
        self.parked_units.insert(left, ctx);

        // Not a reconnect: `track_presence` sees the new unit as it is.
        self.last_presence = device::presence();
        self.last_unit = Some(unit.clone());
        self.restore_device_tabs();
        self.status = format!("Showing {}", device::short_name(&unit));
        if std::mem::take(&mut self.unit_lost) && self.last_presence.available() {
            // Unplugged while parked: what was read may be stale.
            self.device_reconnected(false);
        } else if self.elek_mode {
            self.enter_elek();
        } else if fresh {
            self.preload_elek();
        }
    }

    /// Show `unit` (from a device row or the unit picker).
    pub(super) fn use_unit(&mut self, unit: String) {
        self.device.activate(unit);
        self.sync_unit();
    }

    /// A click in a device row: show its unit (if another), then the
    /// page or +Drive clicked; its name on the shown unit's row switches
    /// that unit off or on.
    pub(super) fn device_row_pick(&mut self, pick: crate::ui::elek_view::RowPick) {
        let other = pick.unit.filter(|u| self.unit_name.as_ref() != Some(u));
        let switched = other.is_some();
        if let Some(u) = other {
            self.use_unit(u);
        }
        match pick.what {
            None if !switched => self.toggle_device(),
            None => {}
            Some(DevicePick::Page(page)) => {
                self.elek.page = page;
                self.enter_full_elek();
            }
            Some(DevicePick::Drive) => {
                // +Drive is a place of its own like Samples / Patterns: the
                // editor in the centre, the +Drive on the right.
                self.elek_mode = false;
                self.show_drive();
            }
        }
    }

    /// State for a unit seen for the first time: nothing read yet, the
    /// view options as they are now.
    fn fresh_unit_ctx(&self) -> UnitCtx {
        let mut ctx = UnitCtx::default();
        ctx.elek.hide_files = self.elek.hide_files;
        ctx.elek.page = self.elek.page;
        ctx.elek.proj.banks = self.elek.proj.banks;
        ctx.device_last = self.device_last;
        ctx
    }

    /// Each device tab's folder, by tab id.
    fn device_tab_dirs(&self) -> std::collections::HashMap<u64, PathBuf> {
        [Role::Source, Role::Dest]
            .iter()
            .flat_map(|&r| self.pane(r).tabs.iter())
            .filter(|t| t.device)
            .map(|t| (t.id, t.cwd.clone()))
            .collect()
    }

    /// Device tabs back in the folders they showed on this unit (its root
    /// where a tab never showed it), listed afresh.
    fn restore_device_tabs(&mut self) {
        for role in [Role::Source, Role::Dest] {
            for i in 0..self.pane(role).tabs.len() {
                let tab = &self.pane(role).tabs[i];
                if !tab.device {
                    continue;
                }
                let dir = self.drive_dirs.get(&tab.id).cloned().unwrap_or_else(|| PathBuf::from("/"));
                let tab = &mut self.pane_mut(role).tabs[i];
                if tab.cwd != dir {
                    tab.navigate_to(dir);
                }
                self.reload(role, i);
            }
        }
    }

    /// Units with Device-view edits not yet pushed (sample pool, patterns,
    /// sound pool, presets), shown unit first: `(unit, changed count)`.
    pub(super) fn unpushed_device_edits(&self) -> Vec<(String, usize)> {
        let count = |e: &ElekState| e.changed() + e.presets.changed();
        let shown = self.unit_name.clone().unwrap_or_else(|| "Device".to_owned());
        let mut parked: Vec<(String, usize)> = self
            .parked_units
            .iter()
            .map(|(u, c)| (u.clone(), count(&c.elek)))
            .collect();
        parked.sort();
        std::iter::once((shown, count(&self.elek)))
            .chain(parked)
            .filter(|(_, n)| *n > 0)
            .collect()
    }

    /// Run `f` with `unit` (not the one shown) standing in as the active
    /// unit: its parked state swapped in and requests going to its worker.
    /// Status lines set meanwhile are marked with its name.
    fn in_unit(&mut self, unit: &str, f: impl FnOnce(&mut Self)) {
        let mut ctx = self.parked_units.remove(unit).unwrap_or_else(|| self.fresh_unit_ctx());
        let shown = device::active_unit();
        self.swap_unit(&mut ctx);
        self.device.route_to(Some(unit.to_owned()));
        let status = self.status.clone();
        self.background = true;
        f(self);
        self.background = false;
        if self.status != status && !self.status.is_empty() {
            self.status = format!("{}: {}", device::short_name(unit), self.status);
        }
        self.device.route_to(shown);
        self.swap_unit(&mut ctx);
        self.parked_units.insert(unit.to_owned(), ctx);
    }

    /// Worker results, each applied to the unit it came from.
    pub(super) fn absorb_units(&mut self, events: Vec<Ev>) {
        for ev in events {
            match ev {
                Ev::FromUnit { unit, ev } if self.unit_name.as_deref() != Some(unit.as_str()) => {
                    self.absorb_background(&unit, *ev)
                }
                Ev::FromUnit { ev, .. } => self.absorb(vec![*ev]),
                ev => self.absorb(vec![ev]),
            }
        }
    }

    /// An event from a unit not shown. Progress is the status bar's, the
    /// rest lands in the unit's parked state.
    fn absorb_background(&mut self, unit: &str, ev: Ev) {
        match ev {
            // For a folder no tab shows any more (the tabs moved to the
            // active unit and were listed again).
            Ev::Listed { .. } => {}
            Ev::Progress { .. } | Ev::Counted { .. } | Ev::Note { .. } => self.absorb(vec![ev]),
            ev => self.in_unit(unit, |app| app.absorb(vec![ev])),
        }
    }
}
