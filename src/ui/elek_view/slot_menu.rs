//! Sample-slot context commands and hover text built from shared row facts.

use super::*;

pub(super) fn slot_menu(ui: &mut egui::Ui, st: &ElekState, row: &SlotRowInfo<'_>, acts: &mut Vec<RowAct>) {
    let Some(p) = st.project.as_ref() else { return };
    let SlotRowInfo { slot, from, entry: e, linked, changed, selected, .. } = *row;
    let n_sel = st.sel.len();
    if !e.is_empty() {
        let (play, open, tip) = if linked.is_some() {
            ("Play local copy", "Load to editor (local copy)", "Nothing is fetched from the device")
        } else {
            ("Fetch and play", "Load to editor…", "Copied into the preview cache first; other fetches wait")
        };
        if ui.add(egui::Button::new(play).shortcut_text("Space")).on_hover_text(tip).clicked() {
            acts.push(RowAct::Local(e.key(), false));
            ui.close();
        }
        if ui.button(open).on_hover_text(tip).clicked() {
            acts.push(RowAct::Local(e.key(), true));
            ui.close();
        }
        ui.separator();
    }
    // Cut / copy: the selection when right-clicking inside it, else this row.
    let pick: Vec<usize> = if selected { st.sel.iter().copied().collect() } else { vec![slot] };
    let filled = pick.iter().filter(|&&s| st.key_at(s).is_some()).count();
    if filled > 0 {
        let what = if filled == 1 { "sample".to_owned() } else { format!("{filled} samples") };
        if ui
            .add(egui::Button::new(format!("Cut {what}")).shortcut_text("⌘X"))
            .on_hover_text("Then paste into this pool to move them; the slots they leave become empty. Nothing changes until you paste")
            .clicked()
        {
            acts.push(RowAct::Clip(pick.clone(), true));
            ui.close();
        }
        if ui
            .add(egui::Button::new(format!("Copy {what}")).shortcut_text("⌘C"))
            .on_hover_text("Then paste into a local file pane to download them there, as when dragging slots out")
            .clicked()
        {
            acts.push(RowAct::Clip(pick.clone(), false));
            ui.close();
        }
    }
    let cut_n = st.clip.as_ref().filter(|c| c.cut).map_or(0, |_| st.clip_positions().len());
    if cut_n > 0 {
        if ui
            .add(egui::Button::new(format!("Paste {cut_n} into empty slots from here")).shortcut_text("⌘V"))
            .on_hover_text("Fills the empty slots from this one down, skipping filled ones; nothing else moves. The cut slots become empty")
            .clicked()
        {
            acts.push(RowAct::Paste(slot, false));
            ui.close();
        }
        if ui
            .add(egui::Button::new(format!("Paste {cut_n} here, shifting down")).shortcut_text("⇧⌘V"))
            .on_hover_text("Inserts at this slot; this and the samples below move down, each into the next empty slot further down. The cut slots become empty")
            .clicked()
        {
            acts.push(RowAct::Paste(slot, true));
            ui.close();
        }
        if ui.button("Cancel cut").clicked() {
            acts.push(RowAct::DropClip);
            ui.close();
        }
    }
    if filled > 0 || cut_n > 0 {
        ui.separator();
    }
    if !e.is_empty() {
        // Only once the device told us where the file lives.
        if let Some(path) = st.names.get(&e.key()) {
            if ui
                .button("Rename on +Drive…")
                .on_hover_text(format!(
                    "{path}\nProjects find samples by content hash, so every project using it keeps working"
                ))
                .clicked()
            {
                acts.push(RowAct::Rename(e.key()));
                ui.close();
            }
        }
    }
    if filled > 0 && ui
        .add(egui::Button::new("Multi-Rename on +Drive…").shortcut_text("⇧F2"))
        .on_hover_text("Find and replace in the selected samples' +Drive names; empty slots are skipped. Local filenames stay unchanged")
        .clicked()
    {
        acts.push(RowAct::MultiRename(pick.clone()));
        ui.close();
    }
    if ui
        .button("Insert empty slot here")
        .on_hover_text(
            "Move this and the rest of the bank down by one, gaps included; an empty slot at the bank's end makes room",
        )
        .clicked()
    {
        acts.push(RowAct::InsertEmpty(slot, 1));
        ui.close();
    }
    // Several selected: make room for as many, above the selection.
    if selected && n_sel > 1 {
        let top = st.sel.first().copied().unwrap_or(slot);
        if ui
            .button(format!("Insert {n_sel} empty slots above selection"))
            .on_hover_text(format!(
                "Move {} and the rest of the bank down by {n_sel}, gaps included; empty slots at the bank's end make room",
                st.label(top)
            ))
            .clicked()
        {
            acts.push(RowAct::InsertEmpty(top, n_sel));
            ui.close();
        }
    }
    if st.free(from as u16)
        && ui
            .button("Remove this empty slot")
            .on_hover_text("Move the rest of the bank up by one, gaps included; the freed slot goes to the bank's end")
            .clicked()
    {
        acts.push(RowAct::CloseGap(slot));
        ui.close();
    }
    if n_sel > 0 {
        ui.separator();
    }
    if n_sel > 0 && !(n_sel == 1 && selected) {
        if ui
            .button(format!("Move {n_sel} selected here"))
            .on_hover_text("Insert the selection above this slot")
            .clicked()
        {
            acts.push(RowAct::MoveHere(slot));
            ui.close();
        }
    }
    if selected {
        if ui.add(egui::Button::new("Move selection up").shortcut_text("⌥↑")).clicked() {
            acts.push(RowAct::Nudge(-1));
            ui.close();
        }
        if ui.add(egui::Button::new("Move selection down").shortcut_text("⌥↓")).clicked() {
            acts.push(RowAct::Nudge(1));
            ui.close();
        }
    }
    // Clearing: the selection when right-clicking inside it, else this row.
    let targets: Vec<usize> = if selected {
        st.sel.iter().copied().filter(|&s| !st.free(st.order[s])).collect()
    } else if !st.free(from as u16) {
        vec![slot]
    } else {
        Vec::new()
    };
    let counted = !p.partial;
    ui.separator();
    // Groups: this row's, or make one from the selection (or this row).
    let mut gacts = Vec::new();
    crate::ui::slot_groups::ui::slot_menu(ui, &st.group_view(), slot, selected, &mut gacts);
    acts.extend(gacts.into_iter().map(RowAct::Group));
    // Navigation and transfers keep the same item/selection targets as before.
    let unfetched: Vec<(u32, u32)> = pick
        .iter()
        .filter_map(|&s| st.key_at(s))
        .filter(|k| !st.linked.contains_key(k) && st.names.contains_key(k))
        .collect();
    let show_original = !e.is_empty() && matches!(linked, Some((_, _, true)));
    let update_changed = !e.is_empty() && changed && st.pool_editable();
    if !unfetched.is_empty() || show_original || update_changed {
        ui.separator();
        // Uploaded from a local file: point at it, so it's clear which
        // of several same-named copies is the one on the device.
        if let Some((_, local, true)) = linked.filter(|_| show_original) {
            if ui
                .button("Show original in source pane")
                .on_hover_text(format!("The file it was uploaded from:\n{}", local.display()))
                .clicked()
            {
                acts.push(RowAct::ShowOriginal(e.key()));
                ui.close();
            }
        }
        if update_changed {
            let all: Vec<(u32, u32)> = st.changed_local.keys().copied().collect();
            if ui
                .button("⬆ Update on device from edited file")
                .on_hover_text("Upload the edited local file next to the +Drive original (which stays, other projects may use it), put it in this slot and push")
                .clicked()
            {
                acts.push(RowAct::UpdateChanged(vec![e.key()]));
                ui.close();
            }
            if all.len() > 1 && ui.button(format!("⬆ Update all {} edited samples", all.len())).clicked() {
                acts.push(RowAct::UpdateChanged(all));
                ui.close();
            }
        }
        if !unfetched.is_empty() {
            let what = if unfetched.len() == 1 { "sample".to_owned() } else { format!("{} samples", unfetched.len()) };
            if ui
                .button(format!("⬇ Fetch {what} for preview"))
                .on_hover_text("Copy into the preview cache in the background, after the fetches already queued, so they play instantly later. Nothing changes on the device")
                .clicked()
            {
                acts.push(RowAct::Fetch(unfetched));
                ui.close();
            }
        }
    }
    if !targets.is_empty() {
        ui.separator();
        let used = targets.iter().filter(|&&s| p.uses[st.order[s] as usize] > 0).count();
        let label = if targets.len() == 1 {
            "Clear sample from project".to_owned()
        } else {
            format!("Clear {} samples from project", targets.len())
        };
        let tip = if used > 0 {
            format!("Unused ones are cleared right away; {used} still used by kits, locks or sounds will ask first")
        } else {
            "Empties the slot in the pool; the file stays on the +Drive. Undo with ⌘Z".to_owned()
        };
        if ui.add_enabled(counted, egui::Button::new(label)).on_hover_text(&tip).on_disabled_hover_text("Wait for the read to finish").clicked() {
            acts.push(RowAct::Clear(targets.clone(), false));
            ui.close();
        }
        let pull_label = if targets.len() == 1 { "Clear and pull others up".to_owned() } else { format!("Clear {} and pull others up", targets.len()) };
        if ui
            .add_enabled(counted, egui::Button::new(pull_label))
            .on_hover_text(format!(
                "Like Clear, then the rest of the bank moves up into each emptied slot, gaps included, so no gap is left there.\n{tip}"
            ))
            .on_disabled_hover_text("Wait for the read to finish")
            .clicked()
        {
            acts.push(RowAct::Clear(targets, true));
            ui.close();
        }
    }
    ui.separator();
    if ui
        .add_enabled(counted, egui::Button::new("Clear all unused samples"))
        .on_hover_text("Empties every slot nothing in the project points at; the files stay on the +Drive. Undo with ⌘Z")
        .on_disabled_hover_text("Wait for the read to finish")
        .clicked()
    {
        acts.push(RowAct::ClearUnused);
        ui.close();
    }
}

pub(super) fn slot_tooltip(st: &ElekState, row: &SlotRowInfo<'_>) -> String {
    let SlotRowInfo { slot, from, entry: e, linked, moved, uses, .. } = *row;
    let mut tip = st
        .names
        .get(&e.key())
        .cloned()
        .unwrap_or_else(|| "(not found on +Drive)".into());
    if let Some((_, local, original)) = linked {
        let cached = crate::links::pool_cache_dir().is_some_and(|c| local.starts_with(c));
        tip.push_str(&format!(
            "\n{}: {}",
            if *original {
                "uploaded from"
            } else if cached {
                "in the preview cache"
            } else {
                "fetched to"
            },
            local.display()
        ));
    } else if st.fetching.contains(&e.key()) {
        tip.push_str("\nqueued for the preview cache");
    }
    if st.fills.contains_key(&(from as u16)) {
        tip.push_str("\nnew in this slot: goes to the device with the next push");
    }
    tip.push_str(&format!("\npool slot {slot}"));
    if moved {
        tip.push_str(&format!(", was {}", st.label(from)));
    }
    if from != slot && uses > 0 {
        tip.push_str("\npatterns using it get renumbered on push");
    }
    tip.push_str(&format!(
        "\nused {uses}× (once per pattern track, plus pool sounds)"
    ));
    tip
}
