//! Track and sound clipboards, paste validation, copy/paste menus and confirmation.

use super::*;

/// A track paste waiting for the user: what it would overwrite.
pub(super) struct TrackPaste {
    pub(super) into: Vec<usize>,
    pub(super) part: Part,
    pub(super) lost: Vec<String>,
    /// From another project (`ProjEd::xtrack_ev` / `xtrack_snd`): the app
    /// does the paste.
    pub(super) cross: bool,
}

/// Copied tracks carried over to another project: their source pattern as
/// edited, with only the samples and sounds the copied parts use.
#[derive(Clone)]
pub struct XTracks {
    pub clip: crate::device::file::PatternClip,
    pub tracks: Vec<u8>,
    /// "TRACK 3 of A01", for messages.
    pub what: String,
}

/// What of a track is copied or pasted: all of it (pasting: whatever is
/// copied), its sound (kit sound, MIDI bit) or its events (trigs, p-locks).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Part {
    Track,
    Sound,
    Events,
}

impl Part {
    fn events(self) -> bool {
        self != Part::Sound
    }
    fn sound(self) -> bool {
        self != Part::Events
    }
}

/// Copied tracks: track → the pattern it comes from (`None`: empty), and the
/// slot they were copied from.
#[derive(Clone)]
pub(super) struct TrackClip {
    pub(super) tracks: Vec<(u8, Option<u8>)>,
    pub(super) from: usize,
    /// Sound overrides as copied, including tag edits and pool pastes.
    pub(super) overrides: std::collections::BTreeMap<u8, SoundCell>,
    pub(super) tags: std::collections::BTreeMap<u8, u32>,
}

/// One sound copied on Pool, Presets or Patterns, shared across all three.
#[derive(Clone)]
pub struct PoolClip {
    /// Remaining sounds in a multi-copy, in selection order.
    pub rest: Vec<PoolClip>,
    /// The pool sound it was copied from (index).
    pub at: usize,
    /// That sound as it was when copied (where it was read from, its name).
    pub cell: SoundCell,
    pub name: String,
    /// Whether it can go onto a kit track (`Err`: why not).
    pub track_ok: Result<(), String>,
    /// The sound as a stand-alone sound struct (like a preset) and the
    /// +Drive key of its sample, so it can be pasted into another project.
    /// `None` for presets, which already are.
    pub carry: Option<(SoundCell, Option<(u32, u32)>)>,
}

impl PoolClip {
    pub fn items(&self) -> Vec<PoolClip> {
        let mut first = self.clone();
        first.rest.clear();
        std::iter::once(first).chain(self.rest.iter().cloned()).collect()
    }

    pub fn desc(&self) -> String {
        if self.carry.is_some() && matches!(self.cell.src, SoundSrc::Preset { .. }) {
            format!("sound “{}” from another project", self.name)
        } else if matches!(self.cell.src, SoundSrc::Preset { .. }) {
            format!("preset “{}”", self.name)
        } else if let SoundSrc::Track { pattern, track } = self.cell.src {
            format!("sound of T{} in {}", track + 1, patterns::label(pattern as usize))
        } else {
            format!("pool sound {:03} “{}”", self.at + 1, self.name)
        }
    }
}

impl ProjEd {
    /// Copy a pool sound (the Pool page): the other clipboards go.
    pub fn clear_sound_clipboard(&mut self) {
        self.tclip_ev = None;
        self.tclip_snd = None;
        self.clip.clear();
        self.xclip = None;
        self.xcopy = None;
        self.xtrack_ev = None;
        self.xtrack_snd = None;
        self.xtcopy = None;
        self.pool_clip = None;
    }

    pub fn set_pool_clips(&mut self, mut clips: Vec<PoolClip>) {
        self.clear_sound_clipboard();
        if !clips.is_empty() {
            let mut first = clips.remove(0);
            first.rest = clips;
            self.pool_clip = Some(first);
        }
    }

    pub fn set_pool_clip(&mut self, c: PoolClip) {
        self.clear_sound_clipboard();
        self.pool_clip = Some(c);
    }

    /// Copied track sounds (track, pattern as read; `None`: an empty
    /// pattern), for the Pool page's paste. Empty when none are copied.
    pub fn copied_sounds(&self) -> Vec<(u8, Option<u8>)> {
        self.tclip_snd.as_ref().map(|c| c.tracks.clone()).unwrap_or_default()
    }

    pub fn copied_sound_cell(&self, track: u8) -> Option<SoundCell> {
        let clip = self.tclip_snd.as_ref()?;
        let mut cell = if let Some(sc) = clip.overrides.get(&track) {
            sc.clone()
        } else {
            clip.tags.get(&track)?;
            let pattern = clip.tracks.iter().find(|(t, _)| *t == track)?.1?;
            SoundCell { src: SoundSrc::Track { pattern, track }, name: None, tags: None }
        };
        if let Some(&tags) = clip.tags.get(&track) { cell.tags = Some(tags); }
        Some(cell)
    }

    pub(super) fn has_tclip(&self) -> bool {
        self.tclip_ev.is_some() || self.tclip_snd.is_some() || self.xtrack_ev.is_some() || self.xtrack_snd.is_some()
    }

    /// Whether `part` has something copied to paste.
    fn can_paste(&self, part: Part) -> bool {
        let local = match part {
            Part::Track => self.tclip_ev.is_some() && self.tclip_snd.is_some(),
            Part::Sound => self.tclip_snd.is_some(),
            Part::Events => self.tclip_ev.is_some(),
        };
        local || !self.local_tclip() && !matches!(self.xclips(part), (None, None))
    }

    /// Tracks copied in this project (rather than carried over from another).
    fn local_tclip(&self) -> bool {
        self.tclip_ev.is_some() || self.tclip_snd.is_some()
    }

    /// The carried-over copies `part` pastes.
    fn xclips(&self, part: Part) -> (Option<&XTracks>, Option<&XTracks>) {
        let ev = self.xtrack_ev.as_ref().filter(|_| part.events());
        let snd = self.xtrack_snd.as_ref().filter(|_| part.sound());
        match part {
            Part::Track if ev.is_none() || snd.is_none() => (None, None),
            _ => (ev, snd),
        }
    }

    /// The copied parts `part` pastes (`Track`: whatever is copied).
    fn clips(&self, part: Part) -> (Option<&TrackClip>, Option<&TrackClip>) {
        (self.tclip_ev.as_ref().filter(|_| part.events()), self.tclip_snd.as_ref().filter(|_| part.sound()))
    }
}

/// Copied tracks, for menus and status lines: "TRACK 3 of A01".
pub(super) fn clip_desc(c: &TrackClip, audio: usize) -> String {
    match c.tracks.as_slice() {
        [(t, _)] => format!("{} of {}", track_name(*t as usize, audio), patterns::label(c.from)),
        t => format!("{} tracks of {}", t.len(), patterns::label(c.from)),
    }
}

/// What pasting `part` would paste: "TRACK 3 of A01", "sound of …", "events of … + sound of …".
fn tclip_desc(ed: &ProjEd, part: Part, audio: usize) -> String {
    if !ed.local_tclip() {
        let from = " from another project";
        return match ed.xclips(part) {
            (Some(e), Some(s)) if e.what == s.what => format!("{}{from}", e.what),
            (Some(e), Some(s)) => format!("events of {} + sound of {}{from}", e.what, s.what),
            (Some(e), None) => format!("events of {}{from}", e.what),
            (None, Some(s)) => format!("sound of {}{from}", s.what),
            (None, None) => "tracks".into(),
        };
    }
    match ed.clips(part) {
        (Some(e), Some(s)) if e.tracks == s.tracks && e.from == s.from => clip_desc(e, audio),
        (Some(e), Some(s)) => format!("events of {} + sound of {}", clip_desc(e, audio), clip_desc(s, audio)),
        (Some(e), None) => format!("events of {}", clip_desc(e, audio)),
        (None, Some(s)) => format!("sound of {}", clip_desc(s, audio)),
        (None, None) => "tracks".into(),
    }
}

pub(super) fn copy_tracks(ed: &mut ProjEd, part: Part, audio: usize) -> String {
    let Some(i) = ed.tsel_pat.filter(|_| !ed.tsel.is_empty()) else {
        return "Select tracks first (click a track name in a folded-out pattern)".into();
    };
    let c = &ed.cells[i];
    let ev = TrackClip { tracks: ed.tsel.iter().map(|&t| (t, c.track_src(t as usize))).collect(), from: i, overrides: Default::default(), tags: Default::default() };
    let snd = TrackClip {
        tracks: ed.tsel.iter().map(|&t| (t, c.sound_src(t as usize))).collect(), from: i,
        overrides: c.pool.iter().filter(|(t, _)| ed.tsel.contains(t)).map(|(&t, s)| (t, s.clone())).collect(),
        tags: c.tags.iter().filter(|(t, _)| ed.tsel.contains(t)).map(|(&t, &tags)| (t, tags)).collect(),
    };
    match part {
        Part::Track => (ed.tclip_ev, ed.tclip_snd) = (Some(ev), Some(snd)),
        Part::Sound => ed.tclip_snd = Some(snd),
        Part::Events => (ed.tclip_ev, ed.tclip_snd) = (Some(ev), None),
    }
    // Carried-over copies follow the same pattern, once built.
    match part {
        Part::Track => (ed.xtrack_ev, ed.xtrack_snd) = (None, None),
        Part::Sound => ed.xtrack_snd = None,
        Part::Events => (ed.xtrack_ev, ed.xtrack_snd) = (None, None),
    }
    ed.xtcopy = Some((i, ed.tsel.clone(), part));
    ed.clip.clear();
    ed.xclip = None;
    ed.xcopy = None;
    ed.pool_clip = None;
    if part == Part::Sound && ed.tsel.len() == 1 {
        let t = *ed.tsel.first().expect("one selected track") as usize;
        if t < audio { ed.sound_copy = Some((i, t)); }
    }
    format!("Copied {}; select a pattern and paste (⌘V or right-click)", tclip_desc(ed, part, audio))
}

/// Paste the copied `part` into `into` (same track numbers). Asks first
/// when pasting events would overwrite tracks with something on them.
pub(super) fn paste_tracks(ed: &mut ProjEd, p: &Patterns, into: Vec<usize>, part: Part, audio: usize) -> String {
    let (ev, snd) = ed.clips(part);
    if ev.is_none() && snd.is_none() && !ed.local_tclip() && !matches!(ed.xclips(part), (None, None)) {
        return paste_xtracks(ed, p, into, part, audio);
    }
    if ev.is_none() && snd.is_none() {
        return match part {
            Part::Track => "No tracks copied yet",
            Part::Sound => "No track sound copied yet",
            Part::Events => "No track events copied yet",
        }
        .into();
    }
    if into.is_empty() {
        return "Select a pattern to paste the tracks into".into();
    }
    if let Some(why) = p.track_copy_blocked() {
        return format!("Can't paste tracks: {why}");
    }
    if ev.into_iter().chain(snd).flat_map(|c| &c.tracks).any(|(_, s)| s.is_none()) && !p.can_clear() {
        return "Can't paste an empty pattern's track: no empty pattern found in this project to copy (see the log)".into();
    }
    let blank = blank_seq(p);
    let mut lost = Vec::new();
    for &i in &into {
        let c = &ed.cells[i];
        for &(t, from) in ev.iter().flat_map(|c| &c.tracks) {
            if c.track_src(t as usize) == from {
                continue;
            }
            let seq = seq_of(p, c, t as usize, &blank);
            let trigs = seq.steps.iter().filter(|s| **s == Step::Note).count();
            let locks = seq.steps.iter().filter(|s| **s == Step::Lock).count();
            let what = match (trigs, locks) {
                (0, 0) if seq.locks.is_empty() => continue,
                (0, 0) => "p-locks".to_owned(),
                (n, 0) => format!("{n} trig(s)"),
                (0, l) => format!("{l} lock trig(s)"),
                (n, l) => format!("{n} trig(s), {l} lock trig(s)"),
            };
            lost.push(format!("{} {}: {what}", patterns::label(i), track_name(t as usize, audio)));
        }
    }
    if lost.is_empty() {
        apply_tracks(ed, &into, part, audio)
    } else {
        let n = lost.len();
        ed.confirm = Some(TrackPaste { into, part, lost, cross: false });
        format!("Pasting would overwrite {n} track(s) with trigs on them: confirm or cancel")
    }
}

/// What pasting carried-over tracks' events into `into` would overwrite.
fn lost_tracks(ed: &ProjEd, p: &Patterns, into: &[usize], tracks: &[u8], audio: usize) -> Vec<String> {
    let blank = blank_seq(p);
    let mut lost = Vec::new();
    for &i in into {
        for &t in tracks {
            let seq = seq_of(p, &ed.cells[i], t as usize, &blank);
            let (trigs, locks) = (seq.steps.iter().filter(|s| **s == Step::Note).count(), seq.steps.iter().filter(|s| **s == Step::Lock).count());
            let what = match (trigs, locks) {
                (0, 0) if seq.locks.is_empty() => continue,
                (0, 0) => "p-locks".to_owned(),
                (n, 0) => format!("{n} trig(s)"),
                (0, l) => format!("{l} lock trig(s)"),
                (n, l) => format!("{n} trig(s), {l} lock trig(s)"),
            };
            lost.push(format!("{} {}: {what}", patterns::label(i), track_name(t as usize, audio)));
        }
    }
    lost
}

/// Paste tracks copied in another project: asks first like `paste_tracks`,
/// then the app matches up their samples and sounds (`ElekIntent::PasteTracks`).
fn paste_xtracks(ed: &mut ProjEd, p: &Patterns, into: Vec<usize>, part: Part, audio: usize) -> String {
    if into.is_empty() {
        return "Select a pattern to paste the tracks into".into();
    }
    if let Some(why) = p.track_copy_blocked() {
        return format!("Can't paste tracks: {why}");
    }
    let tracks: Vec<u8> = ed.xclips(part).0.map(|c| c.tracks.clone()).unwrap_or_default();
    let lost = lost_tracks(ed, p, &into, &tracks, audio);
    if lost.is_empty() {
        request_xtracks(ed, into, part, audio)
    } else {
        let n = lost.len();
        ed.confirm = Some(TrackPaste { into, part, lost, cross: true });
        format!("Pasting would overwrite {n} track(s) with trigs on them: confirm or cancel")
    }
}

/// Hand a carried-over track paste to the app.
pub(super) fn request_xtracks(ed: &mut ProjEd, into: Vec<usize>, part: Part, audio: usize) -> String {
    let (ev, snd) = ed.xclips(part);
    let (events, sound) = (ev.is_some(), snd.is_some());
    let msg = format!("Pasting {}: matching its samples and sounds to this project…", tclip_desc(ed, part, audio));
    ed.want.push(ElekIntent::PasteTracks { into, events, sound });
    msg
}

pub(super) fn apply_tracks(ed: &mut ProjEd, into: &[usize], part: Part, audio: usize) -> String {
    let (ev, snd) = ed.clips(part);
    let (ev, snd) = (ev.cloned(), snd.cloned());
    ed.edit(|cells| {
        for &i in into {
            for &(t, from) in ev.iter().flat_map(|c| &c.tracks) {
                cells[i].set_track(t as usize, from);
            }
            for &(t, from) in snd.iter().flat_map(|c| &c.tracks) {
                if let Some(sc) = snd.as_ref().and_then(|c| c.overrides.get(&t)) {
                    cells[i].set_pool_sound(t as usize, sc.clone());
                } else {
                    cells[i].set_sound(t as usize, from);
                }
                if let Some(&tags) = snd.as_ref().and_then(|c| c.tags.get(&t)) {
                    cells[i].tags.insert(t, tags);
                }
            }
        }
    });
    let at = match into {
        [i] => patterns::label(*i),
        _ => format!("{} patterns", into.len()),
    };
    format!("Pasted {} into {at}", tclip_desc(ed, part, audio))
}

/// Queue a shared sound paste onto kit tracks; resolved after drawing the list.
pub(super) fn request_sound_paste(ed: &mut ProjEd, i: usize, tracks: &[u8]) -> String {
    ed.sound_paste = Some((i, tracks.to_vec()));
    "Preparing sound paste…".into()
}

/// Build the copied patterns' carry-over form (`ProjEd::xclip`) once a
/// pattern copy asked for it, from the project as edited.
pub(super) fn finish_pattern_copy(st: &mut ElekState) {
    finish_track_copy(st);
    let Some(slots) = st.proj.xcopy.take() else { return };
    let Some(proj) = st.project.as_deref() else { return };
    let Ok(p) = proj.patterns.as_deref() else { return };
    let names: Vec<String> = slots.iter().map(|&i| view(p, &st.proj.cells[i]).1).collect();
    let pool = st.new_pool();
    match proj.copy_patterns(None, &slots, &names, &st.order, &st.fills, &st.proj.cells, &st.lib.sounds, &pool, &st.names) {
        Ok(c) => {
            crate::log::line(format!(
                "elek-pat: copied {} for other projects: {} sample(s), {} sound(s), {} sound lock(s) without a sound",
                c.from,
                c.pats.iter().map(|p| p.samples.len()).sum::<usize>(),
                c.pats.iter().map(|p| p.sounds.len()).sum::<usize>(),
                c.lost_sounds
            ));
            st.proj.xclip = Some(c);
        }
        Err(e) => crate::log::line(format!("elek-pat: copy for other projects failed: {e}")),
    }
}

/// Build copied tracks' carry-over form (`ProjEd::xtrack_ev` / `xtrack_snd`).
fn finish_track_copy(st: &mut ElekState) {
    let Some((i, tracks, part)) = st.proj.xtcopy.take() else { return };
    let Some(proj) = st.project.as_deref() else { return };
    let Ok(p) = proj.patterns.as_deref() else { return };
    let audio = proj.model.kit_tracks();
    let name = view(p, &st.proj.cells[i]).1;
    let pool = st.new_pool();
    let only = Some((&tracks, part.events(), part.sound()));
    match proj.copy_patterns(only, &[i], &[name], &st.order, &st.fills, &st.proj.cells, &st.lib.sounds, &pool, &st.names) {
        Ok(clip) => {
            let ts: Vec<u8> = tracks.iter().copied().collect();
            let what = match ts.as_slice() {
                [t] => format!("{} of {}", track_name(*t as usize, audio), patterns::label(i)),
                _ => format!("{} tracks of {}", ts.len(), patterns::label(i)),
            };
            crate::log::line(format!(
                "elek-pat: copied {what} for other projects: {} sample(s), {} sound(s)",
                clip.pats.iter().map(|p| p.samples.len()).sum::<usize>(),
                clip.pats.iter().map(|p| p.sounds.len()).sum::<usize>()
            ));
            let x = XTracks { clip, tracks: ts, what };
            match part {
                Part::Track => (st.proj.xtrack_ev, st.proj.xtrack_snd) = (Some(x.clone()), Some(x)),
                Part::Sound => st.proj.xtrack_snd = Some(x),
                Part::Events => st.proj.xtrack_ev = Some(x),
            }
        }
        Err(e) => crate::log::line(format!("elek-pat: track copy for other projects failed: {e}")),
    }
}

/// A copied sound as a stand-alone sound struct, with the +Drive key of the
/// sample it plays, for `PoolClip::carry`. `None` for presets (already
/// stand-alone) or when it can't be read.
pub(crate) fn carry_sound(st: &ElekState, cell: &SoundCell) -> Option<(SoundCell, Option<(u32, u32)>)> {
    if matches!(cell.src, SoundSrc::Preset { .. }) {
        return None;
    }
    let p = st.project.as_deref()?;
    let data = p.copied_sound_data(cell)?;
    let key = crate::device::presets::sample_key(p.model, &data).or_else(|| {
        let slot = crate::device::library::parse_sound(p.model, 0, &data).slot;
        let e = st.entry(slot);
        (slot > 0 && !e.is_empty()).then(|| e.key())
    });
    Some((SoundCell { src: SoundSrc::Preset { model: p.model, data }, name: None, tags: None }, key))
}

/// Finish sound actions with access to the sample pool, shared by all three pages.
pub(super) fn finish_sound_actions(st: &mut ElekState, ctx: &egui::Context) {
    if let Some((i, t)) = st.proj.sound_copy.take() {
        let result = (|| {
            let proj = st.project.as_deref().ok_or("No project loaded")?;
            let c = st.proj.cells.get(i).ok_or("Pattern not loaded")?;
            let cell = edited_track_cell(c, t).or_else(|| c.sound_src(t).map(|pattern| SoundCell {
                src: SoundSrc::Track { pattern, track: t as u8 }, name: None, tags: None,
            })).ok_or("This track has no sound to copy")?;
            let sound = proj.pool_sound_for_track(&cell)?;
            let carry = carry_sound(st, &cell);
            Ok::<_, String>(PoolClip { at: match cell.src { SoundSrc::Read(o) => o as usize, _ => 0 }, cell, name: sound.name, track_ok: Ok(()), carry, rest: Vec::new() })
        })();
        match result {
            Ok(clip) => {
                let msg = format!("Copied {}: paste into Pool, Presets, or another track on Patterns", clip.desc());
                st.proj.set_pool_clip(clip);
                ctx.copy_text(msg.clone());
                st.status = msg;
            }
            Err(e) => { st.proj.clear_sound_clipboard(); st.status = format!("Cannot copy sound: {e}"); }
        }
    }
    if let Some((i, tracks)) = st.proj.sound_paste.take() {
        let result = st.proj.pool_clip.clone()
            .ok_or_else(|| "Copy a sound on Pool, Presets, or Patterns first".to_owned())
            .and_then(|c| paste_sound_on_tracks(st, c, i, tracks, false));
        st.status = result.unwrap_or_else(|e| format!("Cannot paste sound: {e}"));
        crate::log::line(format!("elek-pat: {}", st.status));
    }
}

/// The pool sound a track was given from the Pool page, decoded: from
/// the pool as read (`pool`) or the pattern track it was pasted from, with
/// its new name.
pub(super) fn pool_cell_sound(p: &Patterns, pool: &[PoolSound], c: &SoundCell) -> Option<PoolSound> {
    let mut s = match &c.src {
        SoundSrc::Read(o) => pool.get(*o as usize)?.clone(),
        SoundSrc::Track { pattern, track } => p.info.get(*pattern as usize)?.tracks.get(*track as usize)?.sound.clone(),
        SoundSrc::Preset { model, data } => crate::device::library::parse_sound(*model, 0, data),
        SoundSrc::Empty => return None,
    };
    if let Some(n) = &c.name {
        s.name = patterns::clean_name(n);
    }
    if let Some(tags) = c.tags { s.tags = tags; }
    Some(s)
}

/// Snapshot the sound source and its staged flags when a track has overrides.
pub(super) fn edited_track_cell(c: &Cell, t: usize) -> Option<SoundCell> {
    let mut sound = if let Some(sc) = c.pool.get(&(t as u8)) {
        sc.clone()
    } else {
        let tags = c.tags.get(&(t as u8))?;
        SoundCell { src: SoundSrc::Track { pattern: c.sound_src(t)?, track: t as u8 }, name: None, tags: Some(*tags) }
    };
    if let Some(&tags) = c.tags.get(&(t as u8)) { sound.tags = Some(tags); }
    Some(sound)
}

/// The Copy track / sound / events items of a track's menu.
pub(super) fn copy_items(ui: &mut egui::Ui, ed: &mut ProjEd, audio: usize) -> Option<String> {
    let mut status = None;
    for (part, text) in [(Part::Track, "Copy track"), (Part::Sound, "Copy track sound"), (Part::Events, "Copy track events")] {
        let tip = match part {
            Part::Track => "Its trigs, p-locks and sound",
            Part::Sound => "The kit sound (sample, parameters, tags); a single sound can also be pasted into Pool or Presets",
            Part::Events => "Trigs, length, scale and p-locks; drops a copied sound",
        };
        let button = egui::Button::new(text).shortcut_text(if part == Part::Track { "⌘C" } else { "" });
        if ui.add(button).on_hover_text(tip).clicked() {
            let msg = copy_tracks(ed, part, audio);
            // ⌘V only arrives when the system clipboard holds text.
            ui.ctx().copy_text(msg.clone());
            status = Some(msg);
            ui.close();
        }
    }
    status
}

/// The Paste track / sound / events items, each greyed out until copied.
pub(super) fn paste_items(ui: &mut egui::Ui, ed: &mut ProjEd, p: &Patterns, into: &[usize], editable: bool, audio: usize) -> Option<String> {
    let mut status = None;
    for (part, text) in [(Part::Track, "Paste track"), (Part::Sound, "Paste track sound"), (Part::Events, "Paste track events")] {
        let ok = ed.can_paste(part);
        let button = egui::Button::new(text).shortcut_text(if part == Part::Track { "⌘V" } else { "" });
        let tip = if ok { format!("Paste {} into the same track numbers", tclip_desc(ed, part, audio)) } else { "Nothing copied for this yet".to_owned() };
        if ui.add_enabled(editable && ok, button).on_hover_text(tip.clone()).on_disabled_hover_text(tip).clicked() {
            status = Some(paste_tracks(ed, p, into.to_vec(), part, audio));
            ui.close();
        }
    }
    // A pool sound goes onto the selected tracks of one pattern.
    if let Some(c) = ed.pool_clip.clone() {
        let at = ed.tsel_pat.filter(|_| !ed.tsel.is_empty());
        let ok = c.track_ok.is_ok() && at.is_some_and(|a| into == [a]);
        let tip = match (&c.track_ok, at) {
            (Err(e), _) => e.clone(),
            (_, Some(a)) if into == [a] => "Onto the selected track(s): the kit sound becomes this pool sound (a MIDI track becomes an audio track)".to_owned(),
            _ => "Select the track(s) to paste onto first (click a track name in a folded-out pattern)".to_owned(),
        };
        ui.separator();
        if ui.add_enabled(editable && ok, egui::Button::new(format!("Paste {}", c.desc())).shortcut_text("⌘V")).on_hover_text(tip.clone()).on_disabled_hover_text(tip).clicked() {
            let ts: Vec<u8> = ed.tsel.iter().copied().collect();
            status = Some(request_sound_paste(ed, into[0], &ts));
            ui.close();
        }
    }
    status
}

/// Asks before a track paste overwrites tracks with trigs. `Some(true)`: go ahead.
pub(super) fn confirm_tracks(ctx: &egui::Context, c: &TrackPaste) -> Option<bool> {
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("elek_track_paste_modal")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading(format!("Overwrite {} track(s)?", c.lost.len()));
        ui.add_space(6.0);
        ui.label("These tracks have something on them that the paste replaces (with their sound and p-locks):");
        ui.add_space(4.0);
        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
            for l in &c.lost {
                ui.label(RichText::new(l).font(oled_font(12.0)));
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new("Nothing is sent until you push; ⌘Z undoes the paste.").weak());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Overwrite").color(theme::DEVICE_ACCENT_BRIGHT)).clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(false);
            }
        });
    });
    answer
}


/// Validate and apply the captured sound paste, asking before adding a sample.
pub(crate) fn paste_sound_on_tracks(st: &mut ElekState, c: PoolClip, i: usize, tracks: Vec<u8>, confirmed: bool) -> Result<String, String> {
    if !c.rest.is_empty() { return Err("Multiple sounds are on the clipboard: paste them on Pool or Presets, or copy one sound for Patterns".into()); }
    if st.edit_locked() { return Err("The device is busy".into()); }
    let proj = st.project.as_deref().ok_or("No project loaded")?;
    let p = proj.patterns.as_deref().map_err(|e| e.clone())?;
    if let Some(why) = p.track_copy_blocked() { return Err(why.to_owned()); }
    if i >= st.proj.cells.len() || !p.bank_loaded(i / PER_BANK) {
        return Err("The destination pattern's bank hasn't fully loaded yet".into());
    }
    let audio = proj.model.kit_tracks();
    let ts: Vec<u8> = tracks.into_iter().filter(|&t| (t as usize) < audio).collect();
    if ts.is_empty() { return Err("Select an audio track to paste a sound onto".into()); }
    let (cell, sample) = crate::ui::elek_library_view::prepare_sound_paste(st, &c)?;
    proj.pool_sound_for_track(&cell)?;
    if let SoundSrc::Track { pattern, .. } = cell.src {
        if (pattern as usize) < PATTERNS && !p.bank_loaded(pattern as usize / PER_BANK) {
            return Err("The source pattern's bank hasn't fully loaded yet".into());
        }
    }
    if !confirmed {
        if let Some(sample) = sample {
            return Ok(crate::ui::elek_library_view::ask_sample_paste(
                st, c, crate::ui::elek_library_view::SoundPasteTarget::Tracks { pattern: i, tracks: ts }, sample,
            ));
        }
    }
    st.proj.edit(|cells| {
        for &t in &ts { cells[i].set_pool_sound(t as usize, cell.clone()); }
    });
    if let Some(why) = st.proj.refused.take() { return Err(why); }
    if let Some((old, entry)) = sample { st.fill(old, entry); }
    let names: Vec<String> = ts.iter().map(|&t| track_name(t as usize, audio)).collect();
    Ok(format!("Pasted {} onto {} of {}.{} Push to send it", c.desc(), names.join(", "), patterns::label(i), if sample.is_some() { " Its source sample was added to the sample pool." } else { "" }))
}
