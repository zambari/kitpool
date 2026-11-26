//! Device-page intent dispatch, project push and prioritized read requests.

use super::*;

impl App {
    pub(in crate::app) fn elek_intents(&mut self, intents: Vec<ElekIntent>) {
        self.sync_pool_fetch();
        for i in intents {
            match i {
                ElekIntent::BackToEditor => {
                    self.elek_mode = false;
                }
                ElekIntent::DeviceRow(pick) => self.device_row_pick(pick),
                ElekIntent::Dock => self.dock_elek(),
                ElekIntent::Undock => {
                    self.elek.page = elek_view::ElekPage::Order;
                    self.enter_full_elek();
                }
                ElekIntent::OpenPatterns => {
                    self.elek.page = elek_view::ElekPage::Project;
                    self.enter_full_elek();
                }
                ElekIntent::FetchAll => self.fetch_all(),
                ElekIntent::FetchKeys(keys) => self.fetch_keys(keys),
                ElekIntent::StopFetch => self.stop_fetch(),
                ElekIntent::PurgeCache => self.purge_cache(),
                ElekIntent::AddFiles(pairs) => self.add_pool_files(pairs),
                // One clipboard: cut / copied slots replace cut / copied files.
                ElekIntent::Clipped => self.clipboard = None,
                ElekIntent::PasteFiles(at) => match self.clipboard.as_ref().filter(|c| !c.device) {
                    Some(c) => {
                        let paths = c.paths.clone();
                        self.elek.paste_files(at, paths);
                    }
                    None => {
                        self.elek.status = "Nothing to paste: cut slots here (⌘X), or copy audio files in a local file pane".into();
                    }
                },
                ElekIntent::ListProjects => {
                    self.elek.projects = None;
                    self.device.elek(ElekCmd::ListProjects);
                }
                ElekIntent::ListPresetBanks => {
                    self.elek.presets.begin_load();
                    self.device.elek(ElekCmd::ListPresetBanks);
                }
                ElekIntent::ListPresets(bank) => {
                    self.elek.presets.begin_load();
                    self.device.elek(ElekCmd::ListPresets(bank));
                }
                ElekIntent::ReadPreset { bank, slot } => {
                    self.elek.presets.begin_load();
                    self.device.elek(ElekCmd::ReadPreset { bank, slot });
                }
                ElekIntent::ReadPresetTags { bank, slot } => {
                    self.device.elek(ElekCmd::ReadPresetTags { bank, slot });
                }
                ElekIntent::PushPresets(plan) => self.device.elek(ElekCmd::PushPresets(plan)),
                ElekIntent::Read => self.elek_read(Vec::new()),
                ElekIntent::ReadSounds => {
                    self.elek.lib.sound_progress = Some((0, crate::device::library::SOUND_POOL));
                    self.device.elek(ElekCmd::ReadSounds);
                }
                ElekIntent::SaveDump => {
                    let Some(p) = &self.elek.project else { continue };
                    let Some(root) = crate::paths::documents("project_dumps") else { continue };
                    match p.save_dump(&root) {
                        Ok(dir) => {
                            crate::log::line(format!("elek: dump saved to {}", dir.display()));
                            self.elek.status = format!("Dump saved to {}", dir.display());
                            reveal(&dir);
                        }
                        Err(e) => {
                            self.elek.status = format!("Couldn't save the dump: {e}");
                            self.errors.push(format!("save dump: {e}"));
                        }
                    }
                }
                ElekIntent::SavePresetDump => {
                    if self.elek.presets.dumping.is_some() { continue; }
                    let Some(root) = crate::paths::documents("preset_dumps") else {
                        self.elek.presets.status = "Couldn't save preset dump: no home folder".into();
                        continue;
                    };
                    self.elek.presets.dumping = Some(("Waiting to read presets".into(), 0, 0));
                    self.device.elek(ElekCmd::SavePresetDump(root));
                }
                ElekIntent::SavePresetFile => {
                    if self.elek.presets.dumping.is_some() { continue; }
                    let Some(path) = Self::ask_save_path("presets", "Presets", "Presets", false) else { continue; };
                    self.elek.presets.dumping = Some(("Waiting to save presets".into(), 0, 0));
                    self.device.elek(ElekCmd::SavePresetFile(path));
                }
                ElekIntent::LoadPresetFile => {
                    if !self.elek.presets.can_load_file() { continue; }
                    let Some(path) = Self::json_dialog("presets", "Presets backup").pick_file() else { continue; };
                    self.elek.presets.importing = true;
                    self.elek.presets.dumping = Some(("Waiting to load presets".into(), 0, 0));
                    self.device.elek(ElekCmd::LoadPresetFile(path));
                }
                ElekIntent::SaveProject(bundle) => self.save_project_file(bundle),
                ElekIntent::LoadProject => self.load_project_file(),
                ElekIntent::SavePattern(slot, name, bundle) => self.save_pattern_file(slot, &name, bundle),
                ElekIntent::LoadPattern(slot) => self.load_pattern_file(slot),
                ElekIntent::PastePatterns(at) => self.paste_pattern_clip(at),
                ElekIntent::PasteTracks { into, events, sound } => self.paste_track_clip(into, events, sound),
                ElekIntent::Local(k, open) => self.elek_local(k, open),
                ElekIntent::UpdateChanged(keys) => self.update_changed(keys),
                ElekIntent::ShowOriginal(k) => match self.links.original(k.0, k.1).map(|l| (l.local.clone(), l.fresh())) {
                    Some((p, fresh)) if p.exists() => {
                        self.show_in_source(&p);
                        self.elek.status = if fresh {
                            format!("{}: the original, shown in the source pane", p.display())
                        } else {
                            format!("{}: changed since upload, so the device plays an older version", p.display())
                        };
                    }
                    Some((p, _)) => self.elek.status = format!("{} was moved or deleted since the upload", p.display()),
                    None => self.elek.status = "No local original known for this sample".into(),
                },
                ElekIntent::RenameFiles(files) => {
                    for (key, from, to) in files {
                        let from = PathBuf::from(from);
                        let target = from.with_file_name(&to);
                        if to.trim().is_empty() || to.contains('/') || to.contains('\\') || target == from {
                            continue;
                        }
                        let job = self.run_device(
                            crate::device::Task::Rename { from, to: target.clone() },
                            "Renaming", 1, 0,
                        );
                        self.pool_rename.insert(job, (key, target.to_string_lossy().into_owned()));
                    }
                }
                ElekIntent::RenameFile { key, from, to } => {
                    let to = to.trim();
                    let from = PathBuf::from(from);
                    let Some(parent) = from.parent() else { continue };
                    let target = parent.join(to);
                    if to.is_empty() || to.contains('/') || target == from {
                        continue;
                    }
                    let job = self.run_device(
                        crate::device::Task::Rename { from, to: target.clone() },
                        "Renaming",
                        1,
                        0,
                    );
                    self.pool_rename.insert(job, (key, target.to_string_lossy().into_owned()));
                    self.relink_after = Some((job, key, target));
                }
                ElekIntent::Push { force } => self.elek_push(force, false),
            }
        }
    }

    /// Send the edits to the device. `live`: an automatic push from Live
    /// mode (always forced, quieter log).
    pub(in crate::app) fn elek_push(&mut self, force: bool, live: bool) {
        let Some(p) = &self.elek.project else { return };
        // A project from a file: the device holds something else
        // entirely, so there's nothing to compare against.
        let whole = p.from_file.is_some();
        let force = force || whole || live;
        let cells = &self.elek.proj.cells;
        let sounds = &self.elek.lib.sounds;
        let msgs = p.build_push(&self.elek.order, &self.elek.fills, cells, sounds);
        let pat_changed = self.elek.proj.changed();
        if pat_changed > 0 {
            for (i, c) in cells.iter().enumerate().filter(|(i, c)| **c != crate::device::Cell::as_read(*i)) {
                crate::log::line(format!("elek-pat: pushing {}: {:?}", crate::device::patterns::label(i), c));
            }
        }
        self.elek.expect_patterns = (pat_changed > 0 || whole).then(|| p.expected_patterns(&self.elek.order, &self.elek.fills, cells, sounds));
        let snd_changed = crate::device::library::sounds_changed(sounds);
        self.elek.expect_sounds = (snd_changed > 0).then(|| {
            for (i, c) in sounds.iter().enumerate().filter(|(i, c)| **c != crate::device::library::SoundCell::as_read(*i)) {
                crate::log::line(format!("elek-lib: pushing pool sound {}: {:?} name {:?}", i + 1, match &c.src {
                    crate::device::library::SoundSrc::Read(o) => format!("from {}", *o as usize + 1),
                    crate::device::library::SoundSrc::Empty => "emptied".into(),
                    crate::device::library::SoundSrc::Preset { .. } => "saved preset".into(),
                    crate::device::library::SoundSrc::Track { pattern, track } => {
                        format!("track {} sound of {}", *track as usize + 1, crate::device::patterns::label(*pattern as usize))
                    }
                }, c.name));
            }
            p.edited_sounds(sounds).into_iter().map(|s| if s.blank { String::new() } else { s.name }).collect()
        });
        for (slot, e) in &self.elek.fills {
            if e.is_empty() {
                crate::log::line(format!("elek: pushing slot {slot} (as read) cleared"));
            } else {
                crate::log::line(format!("elek: pushing new file in slot {slot} (as read): hash {:08x} size {} inode {} seqnr {}", e.hash, e.size, e.inode, e.seqnr));
            }
        }
        if msgs.is_empty() {
            self.elek.status = "Nothing to send: no pattern, kit or sound changes with this order.".into();
            return;
        }
        crate::log::line(format!(
            "elek: {} push, {} of {} dump messages",
            if whole { "whole-project" } else if live { "live" } else { "partial" },
            msgs.len(),
            p.msg_count()
        ));
        let model = p.model;
        let settings = p.settings_data().to_vec();
        self.elek.adopt = None;
        self.elek.keep_blank = p.untouched_blanks(&self.elek.order, &self.elek.fills, cells, sounds);
        self.elek.expect = Some(self.elek.new_pool());
        self.elek.busy = Some((if live { "Live push" } else { "Sending to device" }.into(), 0, msgs.len()));
        self.device.elek(ElekCmd::Push { msgs, settings, force, model });
    }

    /// Live mode: once the pool edits pause and the last push has been read
    /// back in full, push them.
    pub(in crate::app) fn live_push_tick(&mut self, ctx: &egui::Context) {
        const SETTLE: std::time::Duration = std::time::Duration::from_millis(400);
        let st = &mut self.elek;
        if !st.live {
            return;
        }
        let Some(p) = &st.project else {
            st.live = false;
            return;
        };
        if p.read_only.is_some() || p.from_file.is_some() {
            st.live = false;
            return;
        }
        if st.pool_changed() == 0 || !st.filling.is_empty() || st.confirm_push || st.confirm_stale {
            return;
        }
        if let Some(t) = st.edited_at {
            let since = t.elapsed();
            if since < SETTLE {
                ctx.request_repaint_after(SETTLE - since);
                return;
            }
        }
        // A read stopped early: some patterns would be left on the wrong
        // samples, so live pushes stop until a full reload.
        if st.reading.is_none() && st.project.as_ref().is_some_and(|p| p.partial || !p.sounds_read) {
            st.live = false;
            st.status = "Live mode off: the read didn't finish, so not every pattern could follow its samples. Reload to turn it on again.".into();
            return;
        }
        if st.busy.is_some() || !st.pool_pushable() {
            return;
        }
        self.elek_push(true, true);
    }

    /// Patterns page Live: once pattern edits pause, push them and take the
    /// project as sent instead of reading it back. Only pattern-only edits:
    /// sample pool and sound pool changes still go through a normal push.
    pub(in crate::app) fn live_pattern_tick(&mut self, ctx: &egui::Context) {
        const SETTLE: std::time::Duration = std::time::Duration::from_millis(400);
        let st = &mut self.elek;
        if !st.pat_live {
            return;
        }
        let ok = st.project.as_ref().is_some_and(|p| {
            p.read_only.is_none() && p.from_file.is_none() && !p.partial && p.sounds_read
                && p.patterns.as_ref().is_ok_and(|pats| pats.read_only.is_none())
        });
        if !ok {
            // A read under way (a reload) comes back whole: wait for it.
            if st.reading.is_none() {
                st.pat_live = false;
                st.status = "Live patterns off: needs the whole project read from the device".into();
            }
            return;
        }
        if st.pat_seen != st.proj.cells {
            st.pat_seen = st.proj.cells.clone();
            st.pat_edited = Some(std::time::Instant::now());
        }
        if st.proj.changed() == 0 || st.busy.is_some() || st.confirm_push || st.confirm_stale || st.proj.asking() {
            return;
        }
        if st.pool_changed() > 0 || st.lib.sound_changed() > 0 {
            let hint = "Live patterns waiting: sample or sound pool changes are pending; push those first (they need a read-back)";
            if st.status != hint {
                st.status = hint.into();
            }
            return;
        }
        if let Some(t) = st.pat_edited {
            let since = t.elapsed();
            if since < SETTLE {
                ctx.request_repaint_after(SETTLE - since);
                return;
            }
        }
        self.elek_push_patterns();
    }

    /// Send the pattern edits and keep the project built from them, with no
    /// read-back afterwards.
    fn elek_push_patterns(&mut self) {
        let st = &mut self.elek;
        let Some(p) = &st.project else { return };
        let cells = &st.proj.cells;
        let sounds = &st.lib.sounds;
        let msgs = p.build_push(&st.order, &st.fills, cells, sounds);
        let after = match p.after_push(&st.order, &st.fills, cells, sounds) {
            Ok(a) => a,
            Err(e) => {
                st.pat_live = false;
                st.status = format!("Live patterns off: can't build the pushed project here ({e}); push normally");
                return;
            }
        };
        if msgs.is_empty() {
            // Edits that come out the same as read (moved and moved back).
            st.proj.adopt(&after);
            st.pat_seen = st.proj.cells.clone();
            return;
        }
        for (i, c) in cells.iter().enumerate().filter(|(i, c)| **c != crate::device::Cell::as_read(*i)) {
            crate::log::line(format!("elek-pat: live pushing {}: {:?}", crate::device::patterns::label(i), c));
        }
        crate::log::line(format!("elek: live pattern push, {} of {} dump messages, no read-back", msgs.len(), p.msg_count()));
        let model = p.model;
        let settings = p.settings_data().to_vec();
        st.adopt = Some(after);
        st.expect = None;
        st.expect_patterns = None;
        st.expect_sounds = None;
        st.keep_blank.clear();
        st.busy = Some(("Live push".into(), 0, msgs.len()));
        self.device.elek(ElekCmd::Push { msgs, settings, force: true, model });
    }

    /// Read the project from the device. `cached`: empty patterns known
    /// unchanged since the last read (a read-back after a push), not asked
    /// for again; empty for a full read.
    pub(super) fn elek_read(&mut self, cached: Vec<crate::device::DumpMsg>) {
        self.elek.keep_blank.clear();
        self.elek.busy = Some(("Reading project".into(), 0, 129));
        let known = self.elek.names.keys().copied().collect();
        let ctl = std::sync::Arc::new(crate::device::ReadCtl::default());
        ctl.open.store(self.elek.proj.bank_mask(), std::sync::atomic::Ordering::Relaxed);
        self.elek.reading = Some(ctl.clone());
        self.elek.read_shown = false;
        ctl.prioritize(self.elek_read_first());
        self.device.elek(ElekCmd::Read { known, ctl, cached });
    }

    pub(super) fn elek_read_first(&self) -> crate::device::ReadFirst {
        match (self.elek_mode, self.elek.page) {
            (true, elek_view::ElekPage::Project) => crate::device::ReadFirst::Patterns,
            (true, elek_view::ElekPage::Library) => crate::device::ReadFirst::Sounds,
            _ => crate::device::ReadFirst::Names,
        }
    }

    /// A tab switch changes the next read request, including a read already
    /// streaming patterns. The in-flight SysEx request finishes first.
    pub(in crate::app) fn prioritize_elek_read(&self) {
        if let Some(ctl) = &self.elek.reading {
            ctl.prioritize(self.elek_read_first());
        }
    }
}
