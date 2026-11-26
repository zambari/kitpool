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
- `src/device/*`: one worker thread per unit owns that unit's MIDI link.
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
- **Device queue**: one thread per unit (`Worker::run`, `src/device/worker.rs`;
  `Device` routes to the active unit's). Writes
  keep their order; read-only browser requests may jump ahead between steps
  (`jumps_queue`). A sample clicked to play or open goes ahead of waiting
  jobs, not the running one (`App::run_device_next`, `Cmd::Job`'s `first`,
  `worker::park`). Renaming a folder that queued jobs use rewrites their paths
  (`redirect_queue`, `Worker::redirects`). Retries, timeouts and chunk size are
  consts in `src/device/mod.rs`.
- Project reads go through the same queue, re-prioritized every frame by the
  page in view (`App::prioritize_elek_read`, `ReadFirst`). Patterns are read last
  because they're slow.
- Preview fetches (`PoolFetch`) hand the device one file at a time, so the
  status bar (`App::status_bar`) adds the still-queued ones (`PoolFetch::queued`)
  to its file/byte totals and ETA; "Cancel all" also stops that queue.

## Vocabulary

| User says | Means | Code name / start |
|---|---|---|
| **device**, Digitakt, Elektron | The Elektron sampler on USB MIDI. Never the audio output unless they say "output"/"audio device". | `src/device/` |
| **+Drive**, drive (device context) | The sampler's file storage over SysEx. Plain absolute `PathBuf`s that never touch local disk; `DirTab::device == true`. | `device/conn.rs`, `transfer.rs` |
| **drives** (local) | Mounted volumes, `💾 ⏷` menu. | `fs_model::volumes` |
| **Device view**, Elek | Project in the device's memory, shown instead of the editor. | `app/elek.rs`, `ui/elek_view.rs` |
| RAM page (was "Samples") | Sample pool slots, 128 per bank. | `ElekPage::Order` |
| Patterns page | 128 patterns, 16 per bank A–H. | `ElekPage::Project`, `device/elek/patterns.rs` |
| Presets page | Saved sounds on +Drive, `/soundbanks/<bank>/<slot>`. Not part of the project. | `ElekPage::Presets` |
| Pool page | The project's 128 **sound pool** sounds (not the sample pool). Formerly "Sounds". | `ElekPage::Library`, `ui/elek_library_view.rs` |
| **docked pool** | Sample pool shown as the right column (RAM in its switch). | `App::dock_elek`, `show_dest_col` |
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
- **The device rewrites a preset's sample inode on save** (low 16 bits come
  back zero). Preset read-back compares with `same_sound`, which skips the
  inode; hash and size still have to match.
- **Old presets (Digitakt II sound v0) carry a payload tail.** The container
  payload is 1109 bytes like v3, but `decode_sound` returns only the 299-byte
  v0 sound; `preset_container` keeps the rest verbatim when rebuilding from
  the preset's own container, and refuses a foreign template with a tail.
- **Presets status line is logged** (`presets: …` in `KitPool.log`, once per
  change, at the end of `elek_presets_view::show`), so refusals can be traced.
- **Locked presets are refused at staging** (`PresetView::stage_with`): any
  edit that would change a slot locked on the device (by the device's slots,
  not what's shown) is rolled back with a status. Locks only come from listed
  presets, so a truly empty slot is never locked.
- **Device off ≠ unplugged.** Clicking the shown unit's name in its device row
  closes its ports so a DAW can use it (`Presence::Off`, per unit); unplugging
  folds the same panels. Reconnect may be a different unit (`App::last_unit`),
  which drops the old project.
- **Several units plugged in** (`app/units.rs`): one device row each under the
  Sample Editor / Device title (`device_row`, `device_rows_below`). One unit is
  *active* (`device::active_unit`): shown, and sent every request. Each unit has
  its own worker, so switching never waits. The active unit's share of `App`
  (project, page, pool edit, +Drive folders...) is swapped with a parked
  `UnitCtx`; **a field that belongs to one unit goes in the `unit_state!` list**.
  Worker events come back as `Ev::FromUnit`; a parked unit's are applied with its
  state swapped in (`App::in_unit`, `App::background` set: shown tabs left alone).
  Carry-over copies (`xclip`, `xtrack_*`, `pool_clip`) follow the user across
  (`ProjEd::carry_copies`); +Drive file paste across units is refused. A unit is
  its MIDI input name; two units with the same port name show as one.
- A read stopped before the sound pool arrives leaves `sounds_read` false and
  **blocks pool pushes** (`ElekState::pool_pushable`), because pool pushes
  renumber sound locks in every pattern.
- **Two Live modes.** Samples page ⚡ (`ElekState::live`, `live_push_tick`)
  pushes pool edits and reads back in full. Patterns page ⚡
  (`pat_live`, `live_pattern_tick`) pushes pattern-only edits with **no
  read-back**: `ElekProject::after_push` builds the project as sent, kept in
  `ElekState::adopt` until `Pushed`, then `ProjEd::adopt` (keeps selection).
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
- Factory samples can't be copied; the device refuses (`device::FACTORY_LOCKED`).
  A lone request shows the popup; inside a fetch queue (`pool_fetch.total > 1`)
  the file is skipped silently and doesn't count toward the queue's failure
  streak (`App::pool_fetch_finished`'s `skipped`). Other transfers surface it
  once per batch.
- `Panels::default()` is the resting editor layout, but a first run starts on
  `Panels::all_on()` and the intro (`app/intro.rs`, after the first sample
  loads) switches off its way to the default. Change both together.
- Tooltips don't show mid-drag; use `theme::pointer_note`.
- **Slot drags share one path** in `slot_list` (`Grab`, `drop_target_rows`,
  `landing_note`, `paint_landing` after all rows, `drag_autoscroll`); each
  list passes `DropRules` (`PlainDrop`). Samples default to a push-down
  insert (`DropTarget::Shift`, `shift_place`): what's under the block moves
  down only as far as the next empty slots, falling back to a full insert
  when there's no room. Presets default to insert (they're copies, order is
  free). ⌥/⌃ swaps in place in both. Neither list uses the shared target
  for the drop itself: both build a `SlotDrop` in `slot_groups::ui::landing`
  and work it out with `Slots::plan`, which knows groups. There the push
  carries loose empty slots along (gaps are information): it ends on a slot
  the drag lifted, or at the bank's edge, dropping the empties furthest
  from the landing (`plan_picked`'s `place`).
  RAM right-click "Insert empty slot here", "Remove this empty slot" and
  "Clear and pull others up" go through `Slots::plan_insert_empty` /
  `plan_close_gaps` (also the group menu's "Add empty slot above/below"):
  the rest of the bank shifts as one, **gaps included** (empty slots are
  information), using up or freeing a slot at the bank's end; groups move
  whole. (Presets' insert/close still use plain `slot_list`.)
- **Slot groups** are one engine for the RAM and Presets pages
  (`ui/slot_groups.rs`: `Slots` plans over a page's `order`, `GroupHost` is
  how a page applies a `Plan`; `slot_groups/ui.rs` draws frames, minimap,
  jump row, menus and `landing`). Everything below about sample groups holds
  for presets too, except: presets have no slot 0 (`first: 0`), a plain drop
  inserts (`Slots::insert`), slots in unread banks count as filled, and
  members are slot numbers *as shown* (staged edits included), renumbered
  by every staged edit (`groups::renumber`). A preset group edit goes
  through the same `Reorder` staging as any preset move (`commit_plan` →
  `apply_pending` → `stage_with`, one undo step with the groups). Saved per
  unit by bank + slot in `AppState::preset_groups` (`PresetView::base_groups`:
  the layout as of the device, so unpushed moves don't move the save).
  Presets hand hue edits back through `PresetView::hue_edits` (applied to
  `ElekState::group_hues` at the end of `elek_presets_view::show`).
  The page-specific halves: `ui/elek_view/groups.rs` (samples, by +Drive
  hash) and `ui/elek_presets_view/groups.rs`.
- **Sample groups** (`ui/elek_view/groups.rs`) exist only in this app:
  named runs of adjacent slots in one bank (empty ones allowed: a
  *container*, filled first by drops into it), always packed, moving as one
  (dashed frame, name tab; drag by tab or frame sides). Every slot drop is
  planned first and refused if it would split a group or push one past its
  bank's end. Edits that don't know about groups go through `edit`, whose
  `repair_groups` keeps each group's longest packed run. Saved per unit
  (MIDI port name) by +Drive hash in `AppState::pool_groups`, synced by
  `App::sync_pool_groups`; a project gets a group back when those samples
  sit packed in one bank. Empty members save as key `(0, 0)` plus the start slot (`PoolGroup::at`).
  The group right-click menu is `slot_groups::ui::menu`, shown directly on the
  name tab, frame edges and jump buttons, and under `Group “name”` on a member
  row (`slot_groups::ui::slot_menu`). All use the same whole-group actions,
  including "Move to group in this/other bank" with every member, empties included.
  A row's top-level "Move to group" commands still target the clicked slot or
  selection; those item commands are separate from the whole-group submenu.
  A group whose slots are all selected gets a solid frame instead of dashed (`frames`).
  "Group N slots…" (`Slots::new_group`) takes any ungrouped picks in one bank, gaps allowed, and is born
  compact: the picked samples pack into the run that moves them least using only picked or loose empty
  slots (empties left behind, nothing else moves); with no such run it lands like a drop at the first pick.
  "Compact group" is `compactable` / `compact_group`: the samples close up at
  the top, the empty slots fall out just below (an all-empty container keeps one).
  "Add empty … to other banks" is `spread_targets` / `spread_group`.
  "Move toward … in other banks" (group right-click) is `plan_gather`: the
  mean in-bank slot of the same-named groups elsewhere, then the nearest
  legal spot for the group (pushes like a group drop, refused if it spills).
  Arrow keys move a group *held* as a whole (`ElekState::group_held`, set by
  `select_group`: tab, frame, jump button, a group move; cleared once the
  selection stops matching), in `keys.rs` → `step_group` / `plan_step`:
  ↑/↓ one slot, ⇧ or PgUp/PgDn up to 16 (stops short of a neighbour group
  or the bank edge). Passed-over slots rotate to the other side; a one-slot
  step into a neighbour group hops it whole. At a bank edge the next press
  crosses banks (`plan_cross`): lands at the near end of the next bank,
  using that bank's nearest loose empties, refused if short of room or the
  name is taken there.
  A group dropped into a bank that already has its same-named group merges
  into it (`plan_merge` via `plan_join_at`: before it if dropped at/above its
  top, else at its end), refused like a join if it would spill banks.
  ⌥ while dropping a dragged group splits it (`SlotDrop::Split`, `plan_split`):
  the second half becomes a new group of the same name at the drop spot,
  refused if that bank already has one. The landing band previews the half, and the source group shades
  that half while ⌥ is held (`group_frames`).
  A 1px minimap line per group sits in a strip between the rows and the
  scrollbar (`ui::minimap`, `MINIMAP_W`), scaled to the whole bank.
  Each pane has a jump row under its list (`ui::nav_row`, only when its bank
  has groups): a button per group in that bank → `ElekState::go_to_group`
  (selects it, flashes its first slot and eases the pane to it through `align[side]` when it isn't all in view; not `ping_scroll`, which the pane clears before the row draws). Slot drops on a button
  join the group via `plan_join_in_bank` (the join menu uses it too), refused
  when the shuffle would push a slot into a bank other than the group's or
  the picked slots' own. Buttons are in slot order and drag like a group
  tab. A group dropped *between* buttons reorders within its bank via
  `plan_reorder(g, at, cap)`: `Shuffle` is the aggression cap (`Fit`: only
  free room, nothing else moves; `Push`: shove neighbours to the next free
  slot, like a group drop; `Rotate`: slide everything between over by the
  group's length, always fits). It tries every landing spot between the new
  neighbours plus the rotation and keeps the least disturbing (fewest groups
  left touching, so gaps between groups only go when there's no room; then
  fewest other slots moved, their total distance, how far the group falls
  short of the bank's smallest existing gap on each side, its own trip). ⇧
  while dropping caps it at `Fit`.
  Group colour is per *name*, app-wide (all units, banks, projects): a hue
  offset -1..1 (±180° from device yellow) in `AppState::group_hues`, keyed
  by `hue_key` (trimmed, lowercase, like every group name comparison), set by
  the slider beside the rename section of group menus (`hue_slider`), painted via
  `slot_groups::group_color` / `theme::hue_shift`. `elek` is per unit, so
  `App::sync_pool_groups` copies the map in and takes edits back (`hues_dirty`).
  Right-click ▸ "Move to group in this bank" / "…in other bank" is `plan_join`
  (the end-of-group drop with an explicit pick list, not the selection),
  checked via `plan_join_in_bank`, listed per group in `ui::slot_menu` (the
  other-bank list is headed by bank letter).
  Clicking a frame side lines the other pane up on
  its same-named group (`ElekState::align_group`, eased via `align[side]`
  through `ping_scroll`; refused while the panes are scroll-linked).
  The padlock at the top between the two Samples panes
  (`scroll_link_toggle`, `ElekState::scroll_linked`) links their scrolling:
  a pane that moved hands its offset to the other via `follow[side]`, which
  jumps there next draw and doesn't pass the move back. Presets also map filtered rows
  through `RowHit::slot_of` and warn about locked slots.
- Device status and copy/paste feedback goes to the bottom status bar, not to
  banners above lists.

- RAM multi-rename: right-click “Multi-Rename on +Drive…” or ⇧F2 uses
  `elek_view/dialogs.rs::begin_multi_rename` and the shared `rename_view` preview.
  Empty slots are skipped and duplicate paths renamed once. Each +Drive job
  updates its RAM name only on success (`App::pool_rename`, tracked per unit);
  local filenames stay unchanged. The device checks destination collisions
  against its full folder listing before each rename.

## Styling

Context menus follow the same section order where applicable: use/open,
Cut/Copy/Paste, rename and arrangement, locate and transfer, wider bank/project
operations, then clearing/deletion. Keep clipboard commands together and use
`Button::shortcut_text` for shortcuts. Group submenus and headers share
`slot_groups::ui::menu`; do not append whole-group commands at individual call
sites. Keep item/selection targets separate from whole-group targets.

Everything shared is in `src/ui/theme.rs`: palette, accent, device yellow and
wash/frame/dimming, OLED font and colours, shared widgets (`pane_tab`,
`soft_toggle`, `section_header`…). Use `theme::progress_bar` for every bar, and
`theme::readout_text` for dense figure tables (editor Stats). View-
specific colours and metrics are consts at the top of the view file.

## Elsewhere

User-facing behaviour: [README.md](README.md). Device format research:
`project_discovery.md`, `project_management_discovery.md` (local, gitignored).
Folders the app writes to: `src/paths.rs`. What survives restart: `src/state.rs`.


- **Sound Pool multi-selection and groups**: `ui/elek_library_view.rs`
  uses `slot_list::click` (Shift range / command toggle) and the shared
  `slot_groups` planner through `elek_library_view/groups.rs`. Batch moves,
  copies, clearing and tag toggles act on the selection; tag edits apply the
  changed bits while keeping each sound's other tags. Pool batch paste replaces
  an equal-size selection or consecutive slots from one destination
  (`paste_pool_batch`; missing source samples are confirmed once for the
  whole batch, `SoundPasteTarget::PoolBatch`). The shared
  clipboard exposes the first sound to Presets/Patterns; the Pool batch is local
  to the loaded project. Group menus are the shared menus, on group buttons and
  member rows. Groups and cells undo together. Sound groups are session-local;
  reload keeps groups whose members still match at their slots. Empty members
  remain containers, and clearing sounds keeps their group.
