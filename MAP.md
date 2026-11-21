# Map

Only what grep won't tell you: vocabulary, rules, indirections, gotchas.
For "which function draws button X", grep the label. Every file has a `//!`
header saying what it owns; read that first. The old exhaustive
control→function tables are in [MAP_full.md](MAP_full.md) (not maintained).

## Architecture and rules

- `src/app/*`: the `App` struct (`mod.rs`), split by topic across files.
  `logic()` absorbs worker results every frame, even when hidden; `ui()` draws.
- `src/ui/*`: drawing and mouse handling only. **Views take state and return
  intents** (`Intent` file table, `EdIntent` editor, `ElekIntent` device view).
  The `App` method that drew the view handles them, so a button's label is in
  `src/ui/…` and its effect is in `src/app/…`. Main handlers: `App::show_pane`
  (file panes), `App::show_editor` (`app/editing.rs`), `App::elek_intents`
  (`app/elek/commands.rs`).
- `src/editor/*`, `src/audio/*`: editor model and DSP. No egui in `audio`.
- `src/device/*`: one worker thread owns the MIDI link.
- **`ui()` never does I/O.** Post a request, draw from cached state.
- **Everything a worker produces comes back as a `jobs::Ev`**, handled in
  `App::absorb` (`app/transfers.rs`); device-project answers go on to
  `App::elek_event` (`app/elek/events.rs`).
- **Modals are pending fields on `App`**, opened by setting the field and shown
  by `show_pending_dialogs` (`app/pending_dialogs.rs`) after layout. Its call
  order is the same-frame answer order; keep it when adding one.

## Jobs and queues

- Local work: one detached thread per request (`Jobs::spawn` in `src/jobs.rs`).
  Not a queue.
- **Device queue**: one thread (`Worker::run`, `src/device/worker.rs`). Writes
  keep their order; read-only browser requests may jump ahead between steps
  (`jumps_queue`). Renaming a folder that queued jobs use rewrites their paths
  (`redirect_queue`, `Worker::redirects`). Retries, timeouts and chunk size are
  consts in `src/device/mod.rs`.
- Project reads go through the same queue, re-prioritized every frame by the
  page in view (`App::prioritize_elek_read`, `ReadFirst`). Patterns are read last
  because they're slow.

## Vocabulary

| User says | Means | Code name / start |
|---|---|---|
| **device**, Digitakt, Elektron | The Elektron sampler on USB MIDI. Never the audio output unless they say "output"/"audio device". | `src/device/` |
| **+Drive**, drive (device context) | The sampler's file storage over SysEx. Plain absolute `PathBuf`s that never touch local disk; `DirTab::device == true`. | `device/conn.rs`, `transfer.rs` |
| **drives** (local) | Mounted volumes, `💾 ⏷` menu. | `fs_model::volumes` |
| **Device view**, Elek | Project in the device's memory, shown instead of the editor. | `app/elek.rs`, `ui/elek_view.rs` |
| Samples page | Sample pool slots, 128 per bank. | `ElekPage::Order` |
| Patterns page | 128 patterns, 16 per bank A–H. | `ElekPage::Project`, `device/elek/patterns.rs` |
| Presets page | Saved sounds on +Drive, `/soundbanks/<bank>/<slot>`. Not part of the project. | `ElekPage::Presets` |
| Pool page | The project's 128 **sound pool** sounds (not the sample pool). Formerly "Sounds". | `ElekPage::Library`, `ui/elek_library_view.rs` |
| **docked pool** | Sample pool shown as the right column (SAMPLE POOL). | `App::dock_elek`, `show_dest_col` |
| **push** / **read back** | Write edits into device memory / verify them by reading again. | `ElekCmd`, `Conn::push_project`, `app/elek/readback.rs` |
| **SliceKit** | Editor slot collecting slices (K). Code says `kit`/`is_kit`; the Digitakt's own "kit" (a pattern's sound set) is a different thing. | `app/kit.rs`, `editor/clip.rs` |
| **grid kit** | SliceKit on 4/8/16 equal slots; only while the marker padlock is locked. | `editor/grid.rs` |
| **source** / **destination** | Left / right file columns. | `Role::Source`/`Dest`, `src/pane.rs` |
| **commit** | Enter: copy source selection into active destination tab. | `App::commit` |
| **export** | Write the editor's selection to the destination. | `src/audio/export.rs` |
| **pool edit**, update on device | Pool sample opened from its slot; save uploads beside the original, fills the slot, pushes. | `app/pool_edit.rs` |
| **link**, local copy | Device file matched to a local file by the +Drive's content hash. | `src/links.rs` |
| **fingerprint**, print | Our own hash of decoded PCM (several keys per file). Elektron's +Drive hash could not be reproduced. | `src/prints.rs`, `App::pool_match` |
| **preview cache** | Fetched device samples kept by content. | `links::pool_cache_dir` |
| scalpel / map / nums | Editor panels. | `scope` / `overview` / `positions` |
| mono, fold | `Fold::{Mix, Left, Right}` | `audio/stats.rs` |

## Gotchas

- **Saved projects on +Drive are never written** (`proto::FORBIDDEN`). Only
  device memory is pushed. Preset uploads are limited to exact preset paths
  (`allowed_write`).
- **Device off ≠ unplugged.** The DEVICE switch closes ports and stops the
  presence scan so a DAW can use the device (`Presence::Off`); unplugging folds
  the same panels but keeps scanning. Reconnect may be a different unit
  (`App::last_unit`), which drops the old project.
- **Several units plugged in**: the user picks one per run (device badge menu
  or the About box's MIDI panel → `Device::pick`). Not saved. A unit is its
  MIDI input name, paired with the output of the same name. The picked port
  makes the scan report `One(pick)`; unplugging it clears the pick.
- A read stopped before the sound pool arrives leaves `sounds_read` false and
  **blocks pool pushes** (`ElekState::pool_pushable`), because pool pushes
  renumber sound locks in every pattern.
- DT1 answers `0x63` with kit track sounds, so its sound pool comes only from
  the whole dump.
- **Copies carry over to another project.** Pattern payloads point at
  samples and pool sounds by slot only, so each copy also builds a
  carry-over form after ⌘C (`finish_pattern_copy`): patterns →
  `ProjEd::xclip`, tracks → `xtrack_ev` / `xtrack_snd` (source pattern,
  only the copied tracks' samples and sounds), a single sound →
  `PoolClip::carry` (a preset-like sound plus its sample's key). These
  survive `ProjEd::reset`; the in-project clips (`clip`, `tclip_*`) win
  while they exist. Pastes go through `App::take_in_patterns` (shared with
  "Load pattern here…"): re-match samples by +Drive hash, remap sound locks,
  import as hidden sources, then place (`place_imports` / `place_tracks`).
- **One sound clipboard** across Pool, Patterns and Presets (`PoolClip`,
  `ProjEd::set_pool_clip`); the slot clipboard and file clipboard displace each
  other. Copies also put text on the system clipboard, because ⌘V only reaches
  egui when the clipboard holds text.
- A kit never saves over or renames a file its slices came from
  (`Editor::path_is_source`).
- Factory samples can't be copied; the device refuses, surfaced once per run of
  queued jobs (`device::FACTORY_LOCKED`).
- `Panels::default()` is the resting editor layout, but a first run starts on
  `Panels::all_on()` and the intro (`app/intro.rs`, after the first sample
  loads) switches off its way to the default. Change both together.
- Tooltips don't show mid-drag; use `theme::pointer_note`.
- Device status and copy/paste feedback goes to the bottom status bar, not to
  banners above lists.

## Styling

Everything shared is in `src/ui/theme.rs`: palette, accent, device yellow and
wash/frame/dimming, OLED font and colours, shared widgets (`pane_tab`,
`soft_toggle`, `section_header`…). Use `theme::progress_bar` for every bar, and
`theme::readout_text` for dense figure tables (editor Stats). View-
specific colours and metrics are consts at the top of the view file.

## Elsewhere

User-facing behaviour: [README.md](README.md). Device format research:
`project_discovery.md`, `project_management_discovery.md` (local, gitignored).
Folders the app writes to: `src/paths.rs`. What survives restart: `src/state.rs`.
