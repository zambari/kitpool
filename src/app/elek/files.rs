//! Project/pattern JSON files, sample bundles and bundled-sample imports.

use super::*;

/// Patterns taken into the project from a file or another project, before
/// their samples are placed (`App::take_in_patterns`).
#[derive(Default)]
struct TakeIn {
    /// Their source indexes, for `Cell`s.
    idxs: Vec<u8>,
    found: usize,
    /// Samples to place: `(empty slot as read, the sample, a local copy)`.
    lookup: Vec<(u16, file::SampleRef, Option<PathBuf>)>,
    no_room: Vec<String>,
    sounds_found: usize,
    new_sounds: Vec<(usize, crate::device::library::SoundCell)>,
    sounds_no_room: usize,
    cleared: usize,
}

impl App {
    /// Where project and pattern files go by default.
    pub(super) fn elek_files_dir(sub: &str) -> Option<PathBuf> {
        let d = crate::paths::documents(sub)?;
        let _ = std::fs::create_dir_all(&d);
        Some(d)
    }

    pub(super) fn json_dialog(sub: &str, what: &str) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new().add_filter(what, &["json"]);
        if let Some(dir) = Self::elek_files_dir(sub) {
            d = d.set_directory(dir);
        }
        d
    }

    /// Asks where to save: a `.json` file, or for a bundle a folder of that
    /// name (the JSON goes inside it, beside `samples/`).
    pub(super) fn ask_save_path(sub: &str, what: &str, base: &str, bundle: bool) -> Option<PathBuf> {
        if bundle {
            let mut d = rfd::FileDialog::new().set_title(format!("Save {what} with its samples (a folder)")).set_file_name(base);
            if let Some(dir) = Self::elek_files_dir(sub) {
                d = d.set_directory(dir);
            }
            let p = d.save_file()?;
            Some(if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) { p.with_extension("") } else { p })
        } else {
            Self::json_dialog(sub, what).set_file_name(format!("{base}.json")).save_file()
        }
    }

    pub(super) fn save_project_file(&mut self, bundle: bool) {
        let Some(p) = &self.elek.project else { return };
        let base = format!("{} project", p.model.name());
        let Some(path) = Self::ask_save_path("projects", "Project", &base, bundle) else { return };
        let st = &self.elek;
        let file = p.project_file(&st.order, &st.fills, &st.proj.cells, &st.lib.sounds, &st.new_pool(), &st.names);
        self.save_elek_file(file, path, bundle, "Project".into());
    }

    pub(super) fn save_pattern_file(&mut self, slot: usize, name: &str, bundle: bool) {
        let Some(p) = &self.elek.project else { return };
        let label = crate::device::patterns::label(slot);
        let base = if name.trim().is_empty() { label.clone() } else { format!("{label} {}", name.trim()) };
        let base: String = base.chars().map(|c| if "/\\:".contains(c) { '_' } else { c }).collect();
        let Some(path) = Self::ask_save_path("patterns", "Pattern", &base, bundle) else { return };
        let st = &self.elek;
        let file = p.pattern_file(slot, name, &st.order, &st.fills, &st.proj.cells, &st.lib.sounds, &st.new_pool(), &st.names);
        self.save_elek_file(file, path, bundle, label);
    }

    /// Write a built file, or for a bundle gather its samples first:
    /// local copies where we have them, the rest fetched from the device
    /// into the preview cache (`finish_bundle` runs once they're in).
    pub(super) fn save_elek_file(&mut self, file: Result<file::SaveFile, String>, path: PathBuf, bundle: bool, what: String) {
        let mut file = match file {
            Ok(f) => f,
            Err(e) => {
                self.elek.status = format!("Couldn't save {}: {e}", what.to_lowercase());
                self.errors.push(format!("save {}: {e}", what.to_lowercase()));
                return;
            }
        };
        if !bundle {
            match file.write(&path) {
                Ok(()) => {
                    crate::log::line(format!("elek: {what} saved to {}", path.display()));
                    self.elek.status = format!("{what} saved to {}", path.display());
                }
                Err(e) => {
                    self.elek.status = format!("Couldn't save {}: {e}", what.to_lowercase());
                    self.errors.push(format!("save {}: {e}", what.to_lowercase()));
                }
            }
            return;
        }
        if self.pool_fetch.bundle.is_some() {
            self.elek.status = "Another bundle is still gathering its samples; try again once it's saved".into();
            return;
        }
        if path.is_file() {
            self.elek.status = format!("{} is a file; a bundle needs a folder there", path.display());
            return;
        }
        let stem = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "bundle".into());
        let mut waiting = HashSet::new();
        for s in file.samples_mut().iter() {
            let Some(k) = s.key() else { continue };
            if self.links.local(k.0, k.1).is_none() && self.elek.names.contains_key(&k) {
                waiting.insert(k);
            }
        }
        let n = waiting.len();
        let f = &mut self.pool_fetch;
        f.fails = 0;
        for &k in &waiting {
            if f.running.is_some_and(|(_, r)| r == k) || f.queue.contains(&k) {
                continue;
            }
            f.queue.push_back(k);
            if f.total > 0 {
                f.total += 1;
            }
        }
        if f.total == 0 {
            f.done = 0;
            f.total = f.queue.len() + usize::from(f.running.is_some());
        }
        f.bundle = Some(BundleSave { file, json: format!("{stem}.json"), dir: path, what, waiting });
        if n > 0 {
            self.elek.status = format!("Fetching {n} sample(s) from the device for the bundle…");
        }
        // Finishes right away when nothing needs fetching.
        self.pump_fetch();
    }

    /// Drop fetches that are done (or dropped) from the bundle's wait list,
    /// and write it once nothing is left.
    pub(super) fn bundle_progress(&mut self) {
        let f = &self.pool_fetch;
        let pending: HashSet<Key> = f.queue.iter().copied().chain(f.running.map(|(_, k)| k)).collect();
        let Some(b) = self.pool_fetch.bundle.as_mut() else { return };
        b.waiting.retain(|k| pending.contains(k));
        if b.waiting.is_empty() {
            let b = self.pool_fetch.bundle.take().expect("checked above");
            self.finish_bundle(b);
        }
    }

    pub(super) fn finish_bundle(&mut self, mut b: BundleSave) {
        let samples = b.dir.join("samples");
        let what = b.what.clone();
        let fail = |app: &mut Self, e: String| {
            app.elek.status = format!("Couldn't save {}: {e}", what.to_lowercase());
            app.errors.push(format!("save bundle {}: {e}", b.dir.display()));
        };
        if let Err(e) = std::fs::create_dir_all(&samples) {
            return fail(self, format!("{}: {e}", samples.display()));
        }
        let mut taken = HashSet::new();
        let mut done: HashMap<Key, String> = HashMap::new();
        let (mut copied, mut missing, mut failed) = (0, Vec::new(), Vec::new());
        for s in b.file.samples_mut() {
            let Some(k) = s.key() else { continue };
            if let Some(f) = done.get(&k) {
                s.file = Some(f.clone());
                continue;
            }
            let Some(l) = self.links.local(k.0, k.1) else {
                missing.push(s.short());
                continue;
            };
            let src = l.local.clone();
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("wav").to_lowercase();
            let name = file::bundle_name(s, l.device_name(), &ext, &mut taken);
            match std::fs::copy(&src, samples.join(&name)) {
                Ok(_) => {
                    let rel = format!("samples/{name}");
                    s.file = Some(rel.clone());
                    done.insert(k, rel);
                    copied += 1;
                }
                Err(e) => failed.push(format!("{}: {e}", src.display())),
            }
        }
        let json = b.dir.join(&b.json);
        if let Err(e) = b.file.write(&json) {
            return fail(self, e);
        }
        crate::log::line(format!(
            "elek: {what} bundle saved to {}: {copied} sample(s), missing {missing:?}, failed {failed:?}",
            b.dir.display()
        ));
        let mut msg = format!("{what} saved with {copied} sample(s) to {}", b.dir.display());
        if !missing.is_empty() {
            msg.push_str(&format!(". No copy of {} (not found on the device): noted by hash only", missing.join(", ")));
        }
        if !failed.is_empty() {
            msg.push_str(&format!(". {} couldn't be copied (see errors)", failed.len()));
            self.errors.extend(failed);
        }
        self.elek.status = msg;
        reveal(&json);
    }

    /// A load is still placing samples from its file.
    pub(super) fn importing(&self) -> bool {
        self.pool_fetch.import.as_ref().is_some_and(|i| i.slots.keys().any(|s| self.elek.filling.contains_key(s)))
    }

    /// +Drive folder for samples a bundle brings that the device lacks.
    pub(super) fn import_dir(json: &Path) -> PathBuf {
        let stem = json.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "bundle".into());
        let stem: String = stem.chars().map(|c| if "/\\".contains(c) { '_' } else { c }).collect();
        PathBuf::from("/imported").join(stem.trim())
    }

    /// Start placing `slots` (`(slot as read, its sample, a local copy to
    /// upload if the device lacks it)`): each is looked up on the device by
    /// hash, and uploaded into `dir` if missing. `verify`: the slot already
    /// points at the file (a loaded project); otherwise it's an empty slot
    /// to fill.
    pub(super) fn start_import(&mut self, what: String, dir: PathBuf, slots: Vec<(u16, file::SampleRef, Option<PathBuf>)>, verify: bool) {
        let mut imp = Import { what, ..Default::default() };
        for (slot, s, file) in slots {
            let Some(key) = s.key() else { continue };
            let name = s.short();
            self.elek.filling.insert(slot, name.clone());
            imp.slots.insert(slot, ImportSlot { name, key, file, dir: dir.clone(), stage: Stage::Hash, verify });
            self.device.elek(ElekCmd::Resolve { slot, hash: key.0, lens: vec![key.1] });
        }
        let empty = imp.slots.is_empty();
        self.pool_fetch.import = Some(imp);
        if empty {
            self.import_summary();
        }
    }

    pub(super) fn import_resolved(&mut self, slot: u16, hash: u32, found: Result<Option<(u32, String)>, String>) {
        let Some(is) = self.pool_fetch.import.as_mut().and_then(|i| i.slots.get_mut(&slot)) else { return };
        if !self.elek.filling.contains_key(&slot) {
            // The project was closed or replaced meanwhile.
            self.pool_fetch.import = None;
            return;
        }
        let name = is.name.clone();
        match found {
            Ok(Some((len, path))) => {
                let e = crate::device::PoolEntry::new_file(hash, len);
                let up = is.stage != Stage::Hash;
                if up && e.key() != is.key {
                    crate::log::line(format!(
                        "elek: {name} came back as {:08x}/{} (bundle had {:08x}/{})",
                        e.hash, len, is.key.0, is.key.1
                    ));
                }
                let same = is.verify && self.elek.entry(slot as usize).key() == e.key();
                self.elek.names.insert(e.key(), path);
                if !same {
                    self.elek.fill(slot, e);
                }
                let imp = self.pool_fetch.import.as_mut().expect("checked above");
                if up { imp.uploaded += 1 } else { imp.found += 1 }
                self.import_done(slot, None);
                self.refresh_elek_links();
            }
            Ok(None) => match is.stage {
                Stage::Hash => {
                    let Some(file) = is.file.clone() else {
                        if let Some(i) = self.pool_fetch.import.as_mut() {
                            i.missing.push(name);
                        }
                        return self.import_done(slot, None);
                    };
                    // Uploaded from this very file before, under another hash?
                    let link = self.links.local_link(&file).map(|l| (l.hash, l.size)).filter(|&(h, _)| h != is.key.0);
                    if let Some((h, size)) = link {
                        is.stage = Stage::Link;
                        self.resolve_fill(slot, h, size);
                    } else {
                        is.stage = Stage::Upload;
                        let dir = is.dir.clone();
                        self.import_upload(slot, file, dir);
                    }
                }
                Stage::Link => {
                    is.stage = Stage::Upload;
                    let (file, dir) = (is.file.clone().expect("linked from it"), is.dir.clone());
                    self.import_upload(slot, file, dir);
                }
                Stage::Upload => self.import_done(slot, Some(format!("{name}: uploaded, but the device can't find it by its hash"))),
            },
            Err(e) => self.import_done(slot, Some(format!("{name}: {e}"))),
        }
    }

    pub(super) fn import_upload(&mut self, slot: u16, file: PathBuf, dir: PathBuf) {
        let bytes = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        let short = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let task = device::Task::Transfer {
            dir: device::Dir::Upload,
            items: vec![(file.clone(), dir.clone())],
            policy: Policy::KeepBoth,
            mono: None,
            total_bytes: bytes,
            local_names: Default::default(),
        };
        let id = self.run_device(task, &format!("Uploading {short} for the pool"), 1, bytes);
        crate::log::line(format!("elek: {short} not on the device; uploading to {}", device::dev_str(&dir)));
        self.pool_fetch.uploads.insert(id, vec![(slot, file)]);
    }

    /// Slot `slot` is placed (or failed with `err`); sums up once all are.
    pub(super) fn import_done(&mut self, slot: u16, err: Option<String>) {
        self.elek.filling.remove(&slot);
        let Some(imp) = self.pool_fetch.import.as_mut() else { return };
        imp.slots.remove(&slot);
        if let Some(e) = err {
            imp.failed.push(e);
        }
        if imp.slots.is_empty() {
            self.import_summary();
        } else {
            let left = imp.slots.len();
            self.elek.status = format!("{}: placing samples, {left} to go…", imp.what);
        }
    }

    pub(super) fn import_summary(&mut self) {
        let Some(imp) = self.pool_fetch.import.take() else { return };
        let mut msg = imp.what.clone();
        let mut parts = Vec::new();
        if imp.found > 0 {
            parts.push(format!("{} sample(s) found on the device", imp.found));
        }
        if imp.uploaded > 0 {
            parts.push(format!("{} uploaded from the bundle", imp.uploaded));
        }
        if !parts.is_empty() {
            msg.push_str(&format!(": {}", parts.join(", ")));
        }
        if !imp.missing.is_empty() {
            msg.push_str(&format!(". Not on the device and not bundled: {}", imp.missing.join(", ")));
        }
        if !imp.failed.is_empty() {
            msg.push_str(&format!(". {} sample(s) failed (see errors)", imp.failed.len()));
        }
        crate::log::line(format!("elek: import done: {msg}; failed {:?}", imp.failed));
        self.errors.extend(imp.failed);
        self.elek.status = msg;
    }

    pub(super) fn load_project_file(&mut self) {
        if self.elek.changed() > 0 {
            self.elek.status = "Push or close the current project first: it has unpushed changes".into();
            return;
        }
        if self.importing() {
            self.elek.status = "Still placing samples from the last load; try again once it's done".into();
            return;
        }
        let Some(path) = Self::json_dialog("projects", "Project").pick_file() else { return };
        match crate::device::ElekProject::load_file(&path) {
            Ok((p, samples)) => {
                crate::log::line(format!(
                    "elek: project loaded from {}: {}{}",
                    path.display(),
                    p.versions,
                    p.read_only.as_ref().map(|w| format!(", READ-ONLY: {w}")).unwrap_or_default()
                ));
                let n = p.pool.len();
                let st = &mut self.elek;
                st.names.extend(samples.iter().filter_map(|s| Some((s.key()?, s.path.clone()?))));
                st.set_project(p);
                self.refresh_elek_links();
                let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let bundled = samples.iter().filter(|s| s.bundled(&path).is_some()).count();
                let slots: Vec<_> = samples.into_iter().filter(|s| s.slot > 0 && s.slot < n).map(|s| {
                    let file = s.bundled(&path);
                    (s.slot as u16, s, file)
                }).collect();
                let what = format!(
                    "Opened {file_name}{}. It's only here for now: Push sends the whole project into the device's memory",
                    if bundled > 0 { format!(" ({bundled} bundled sample(s))") } else { String::new() }
                );
                self.elek.status = format!("{what}. Checking its samples on the device…");
                self.start_import(what, Self::import_dir(&path), slots, true);
            }
            Err(e) => {
                self.elek.status = format!("Couldn't open the project: {e}");
                self.errors.push(format!("load project {}: {e}", path.display()));
            }
        }
    }

    /// Load a pattern file into `slot`. Its samples are matched to this pool
    /// by +Drive hash; ones not in the pool go into empty slots, each found
    /// on the device by hash or, from a bundle, uploaded (push waits).
    pub(super) fn load_pattern_file(&mut self, slot: usize) {
        if self.importing() {
            self.elek.status = "Still placing samples from the last load; try again once it's done".into();
            return;
        }
        let Some(path) = Self::json_dialog("patterns", "Pattern").pick_file() else { return };
        let Some(p) = &self.elek.project else { return };
        let lp = match p.load_pattern(&path) {
            Ok(lp) => lp,
            Err(e) => {
                self.elek.status = format!("Couldn't load the pattern: {e}");
                return;
            }
        };
        let files = lp.samples.iter().filter_map(|s| Some((s.key()?, s.bundled(&path)?))).collect();
        let what = format!("Loaded \"{}\" into {}", lp.name, crate::device::patterns::label(slot));
        crate::log::line(format!("elek-pat: loading {} into {}", path.display(), crate::device::patterns::label(slot)));
        self.place_patterns(slot, vec![lp], files, Self::import_dir(&path), what, false);
    }

    /// Paste patterns copied in another project (`ProjEd::xclip`) from
    /// `at` on. Samples the device lacks (deleted since the copy) are
    /// uploaded from a local copy when there is one.
    pub(super) fn paste_pattern_clip(&mut self, at: usize) {
        if self.importing() {
            self.elek.status = "Still placing samples from the last paste or load; try again once it's done".into();
            return;
        }
        if self.elek.edit_locked() {
            self.elek.status = "The device is busy; paste once it's done".into();
            return;
        }
        let Some(c) = self.elek.proj.xclip.clone() else { return };
        let Some(p) = &self.elek.project else { return };
        if c.model != p.model {
            self.elek.status = format!("Can't paste: {} was copied from a {} project; this one is for the {}", c.from, c.model.name(), p.model.name());
            return;
        }
        let files = c
            .pats
            .iter()
            .flat_map(|lp| &lp.samples)
            .filter_map(|s| {
                let k = s.key()?;
                Some((k, self.links.local(k.0, k.1)?.local.clone()))
            })
            .collect();
        let n = c.pats.len().min(crate::device::patterns::PATTERNS - at);
        let what = match n {
            1 => format!("Pasted {} into {}", c.from, crate::device::patterns::label(at)),
            _ => format!("Pasted {} into {}–{}", c.from, crate::device::patterns::label(at), crate::device::patterns::label(at + n - 1)),
        };
        let mut what = what;
        if c.lost_sounds > 0 {
            what.push_str(&format!(" ({} sound lock(s) dropped: the source's sound pool wasn't read)", c.lost_sounds));
        }
        crate::log::line(format!("elek-pat: pasting {} from another project at {}", c.from, crate::device::patterns::label(at)));
        self.place_patterns(at, c.pats.into_iter().take(n).collect(), files, PathBuf::from("/imported/pasted"), what, true);
    }

    /// Put `pats` into the slots from `at` on (see `take_in_patterns`).
    fn place_patterns(&mut self, at: usize, pats: Vec<file::LoadedPattern>, files: HashMap<Key, PathBuf>, dir: PathBuf, what: String, sounds: bool) {
        let Some(t) = self.take_in_patterns(&pats, &files, sounds) else { return };
        if let Err(why) = self.elek.proj.place_imports(at, &t.idxs) {
            self.elek.status = format!("Couldn't paste: {why}");
            return;
        }
        self.finish_take_in(t, what, dir);
    }

    /// Paste tracks copied in another project (`ProjEd::xtrack_ev` /
    /// `xtrack_snd`) into patterns `into`, same track numbers: their events
    /// (`events`) and / or sounds (`sound`), with samples and sounds matched
    /// up as for a pattern paste.
    pub(super) fn paste_track_clip(&mut self, into: Vec<usize>, events: bool, sound: bool) {
        if self.importing() {
            self.elek.status = "Still placing samples from the last paste or load; try again once it's done".into();
            return;
        }
        if self.elek.edit_locked() {
            self.elek.status = "The device is busy; paste once it's done".into();
            return;
        }
        let ev = self.elek.proj.xtrack_ev.clone().filter(|_| events);
        let snd = self.elek.proj.xtrack_snd.clone().filter(|_| sound);
        let Some(p) = &self.elek.project else { return };
        if let Some(c) = ev.iter().chain(&snd).find(|c| c.clip.model != p.model) {
            self.elek.status = format!("Can't paste: {} was copied from a {} project; this one is for the {}", c.what, c.clip.model.name(), p.model.name());
            return;
        }
        // One source pattern per clip; both are the same when a whole track was copied.
        let same = matches!((&ev, &snd), (Some(e), Some(s)) if e.tracks == s.tracks
            && e.clip.pats.first().map(|p| &p.data) == s.clip.pats.first().map(|p| &p.data));
        let mut pats = Vec::new();
        pats.extend(ev.iter().flat_map(|c| c.clip.pats.first().cloned()));
        if !same {
            pats.extend(snd.iter().flat_map(|c| c.clip.pats.first().cloned()));
        }
        if pats.is_empty() {
            return;
        }
        let files = pats
            .iter()
            .flat_map(|lp| &lp.samples)
            .filter_map(|s| {
                let k = s.key()?;
                Some((k, self.links.local(k.0, k.1)?.local.clone()))
            })
            .collect();
        let Some(t) = self.take_in_patterns(&pats, &files, true) else { return };
        let ev_idx = ev.as_ref().map(|_| t.idxs[0]);
        let snd_idx = snd.as_ref().map(|_| if same || ev.is_none() { t.idxs[0] } else { t.idxs[1] });
        let ev_tracks = ev.as_ref().map(|c| c.tracks.clone()).unwrap_or_default();
        let snd_tracks = snd.as_ref().map(|c| c.tracks.clone()).unwrap_or_default();
        if let Err(why) = self.elek.proj.place_tracks(&into, ev_idx, &ev_tracks, snd_idx, &snd_tracks) {
            self.elek.status = format!("Couldn't paste: {why}");
            return;
        }
        let what = match (&ev, &snd) {
            (Some(e), Some(_)) if same => e.what.clone(),
            (Some(e), Some(s)) => format!("events of {} + sound of {}", e.what, s.what),
            (Some(e), None) => format!("events of {}", e.what),
            (None, Some(s)) => format!("sound of {}", s.what),
            (None, None) => "tracks".into(),
        };
        let at = match into.as_slice() {
            [i] => crate::device::patterns::label(*i),
            _ => format!("{} patterns", into.len()),
        };
        crate::log::line(format!("elek-pat: pasting {what} from another project into {at}"));
        self.finish_take_in(t, format!("Pasted {what} into {at}"), PathBuf::from("/imported/pasted"));
    }

    /// Take `pats` into the project as hidden sources (not yet in a slot).
    /// Their samples are matched to this pool by +Drive hash; others get
    /// empty slots, to be found on the device by hash or else uploaded from
    /// `files` (`finish_take_in`; push waits). With `sounds` (a paste), pool
    /// sounds their sound locks play are matched to this sound pool (an
    /// identical sound) or go into empty pool sounds; locks with no sound to
    /// play are cleared. Pattern files carry no sounds: their locks are left
    /// as they are. `None`: refused, with the status set.
    fn take_in_patterns(&mut self, pats: &[file::LoadedPattern], files: &HashMap<Key, PathBuf>, sounds: bool) -> Option<TakeIn> {
        use crate::device::library::{self, SoundCell, SoundSrc, SOUND_POOL};
        let st = &self.elek;
        let p = st.project.as_ref()?;
        let n = p.pool.len();
        let locked = sounds && pats.iter().any(|lp| !p.payload_sound_locks(&lp.data).is_empty());
        if locked && !(p.sounds_read && p.locks_known()) {
            self.elek.status = if p.sounds_read {
                "Can't paste: the copied pattern has sound locks, and this project's sound locks can't be renumbered (unknown pattern layout)".into()
            } else {
                "Can't paste yet: what was copied has sound locks, and this project's sound pool hasn't been read. Read it on the Pool page, then paste again".into()
            };
            return None;
        }
        let cells: Vec<SoundCell> = if st.lib.sounds.len() == SOUND_POOL { st.lib.sounds.clone() } else { library::sounds_as_read() };
        let mut t = TakeIn::default();
        let mut taken = HashSet::new();
        let mut found_keys = HashSet::new();
        let mut maps = Vec::new();
        // Per pattern (each may come from another project, numbered its own way).
        for lp in pats {
            // Samples: slot in the source → slot as read here.
            let mut map = HashMap::new();
            for s in &lp.samples {
                let Some(k) = s.key() else { continue };
                // Already in the pool.
                if let Some(o) = (1..n).find(|&o| {
                    let e = st.entry(o);
                    !e.is_empty() && e.key() == k
                }) {
                    map.insert(s.slot, o);
                    if found_keys.insert(k) {
                        t.found += 1;
                    }
                    continue;
                }
                // On its way in from this same paste.
                if let Some((o, _, _)) = t.lookup.iter().find(|(_, x, _)| x.key() == Some(k)) {
                    map.insert(s.slot, *o as usize);
                    continue;
                }
                // The first empty slot in the order as shown.
                match st.order.iter().skip(1).copied().find(|&o| st.free(o) && !taken.contains(&o)) {
                    Some(o) => {
                        taken.insert(o);
                        map.insert(s.slot, o as usize);
                        t.lookup.push((o, s.clone(), files.get(&k).cloned()));
                    }
                    None => t.no_room.push(s.short()),
                }
            }
            // Sounds: lock value in the source → pool sound as read here.
            let mut smap: HashMap<u8, u8> = HashMap::new();
            if locked {
                for (v, data) in &lp.sounds {
                    if smap.contains_key(v) {
                        continue;
                    }
                    let mut d = data.clone();
                    let s = library::parse_sound(p.model, 0, &d).slot;
                    if s > 0 {
                        crate::device::presets::translate_slot(p.model, &mut d, map.get(&s).copied().unwrap_or(0));
                    }
                    // The same sound already in this pool, where it was read,
                    // or added by this paste.
                    let have = (0..SOUND_POOL)
                        .find(|&i| cells[i] == SoundCell::as_read(i) && p.sound_as_read(i).as_deref() == Some(&d[..]))
                        .or_else(|| t.new_sounds.iter().find(|(_, c)| matches!(&c.src, SoundSrc::Preset { data, .. } if *data == d)).map(|(i, _)| *i));
                    if let Some(i) = have {
                        smap.insert(*v, i as u8);
                        t.sounds_found += 1;
                        continue;
                    }
                    // An empty pool sound whose locks point nowhere else.
                    let free = (0..SOUND_POOL).find(|&i| {
                        let empty = match cells[i].src {
                            SoundSrc::Empty => true,
                            SoundSrc::Read(o) => o as usize == i && p.sound_as_read(i).is_none(),
                            _ => false,
                        };
                        empty
                            && !t.new_sounds.iter().any(|(j, _)| *j == i)
                            && !cells.iter().enumerate().any(|(j, c)| j != i && c.src == SoundSrc::Read(i as u8))
                    });
                    match free {
                        Some(i) => {
                            smap.insert(*v, i as u8);
                            t.new_sounds.push((i, SoundCell { src: SoundSrc::Preset { model: p.model, data: d }, name: None, tags: None }));
                        }
                        None => t.sounds_no_room += 1,
                    }
                }
            }
            maps.push((map, smap));
        }
        // Take the patterns in.
        let p = self.elek.project.as_mut()?;
        for (lp, (map, smap)) in pats.iter().zip(&maps) {
            let mut data = lp.data.clone();
            if locked {
                t.cleared += p.remap_sound_locks(&mut data, smap);
            }
            match p.import_pattern(data, map) {
                Ok(i) => t.idxs.push(i),
                Err(e) => {
                    self.elek.status = format!("Couldn't paste \"{}\": {e}", lp.name);
                    return None;
                }
            }
        }
        crate::log::line(format!(
            "elek-pat: took in {} pattern(s): {} sample(s) in the pool, {} to place, {} without room; maps {maps:?}; \
             sounds {} found, {} added, {} without room, {} locked step(s) cleared",
            t.idxs.len(),
            t.found,
            t.lookup.len(),
            t.no_room.len(),
            t.sounds_found,
            t.new_sounds.len(),
            t.sounds_no_room,
            t.cleared
        ));
        Some(t)
    }

    /// After the taken-in patterns are placed: add their pool sounds, sum
    /// up in the status line, and start placing their samples.
    fn finish_take_in(&mut self, t: TakeIn, what: String, dir: PathBuf) {
        if !t.new_sounds.is_empty() {
            let cells = self.elek.lib.cells_mut();
            for (i, c) in &t.new_sounds {
                cells[*i] = c.clone();
            }
        }
        self.elek.lib.patterns_changed();
        let mut msg = what;
        if t.found > 0 {
            msg.push_str(&format!(" ({} sample(s) already in the pool)", t.found));
        }
        if !t.new_sounds.is_empty() {
            let at: Vec<String> = t.new_sounds.iter().map(|(i, _)| format!("{:03}", i + 1)).collect();
            msg.push_str(&format!(". Sound-locked sounds added to the sound pool at {}", at.join(", ")));
        }
        if !t.no_room.is_empty() {
            msg.push_str(&format!(". No empty pool slot for {}: those tracks play nothing", t.no_room.join(", ")));
        }
        if t.sounds_no_room > 0 || t.cleared > 0 {
            msg.push_str(&format!(". No empty pool sound for {} locked sound(s): {} sound-locked step(s) cleared", t.sounds_no_room, t.cleared));
        }
        msg.push_str(". Push to send");
        if !t.lookup.is_empty() {
            self.elek.status = format!("{msg}. Placing {} sample(s)…", t.lookup.len());
        } else {
            self.elek.status = msg.clone();
        }
        self.start_import(msg, dir, t.lookup, false);
    }
}
