//! The KitPool title button and About popup: build details, links, the MIDI
//! ports the system offers (the device's marked "(using)"), and Reset state.

const POPUP_W: f32 = 540.0;
const PAD: i8 = 28;
/// The MIDI box's text: secondary detail, kept below the version and links.
const MIDI_TEXT: f32 = 11.0;

/// What the About popup asks the app to do.
pub enum AboutIntent {
    /// Forget the saved state and restart as on first run.
    ResetState,
}

/// `ports` is only called while the popup is open. `can_reset` is false
/// while a transfer runs.
pub fn show(
    ui: &mut egui::Ui,
    ports: fn() -> crate::device::MidiPorts,
    can_reset: bool,
) -> Option<AboutIntent> {
    let mut out = None;
    let about = ui
        .add(
            egui::Button::new(egui::RichText::new("KITPOOL").size(13.0).strong())
                .frame(false),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("About KitPool");
    let confirm_id = about.id.with("reset_confirm");
    let shown = egui::Popup::menu(&about)
        .width(POPUP_W)
        .frame(egui::Frame::popup(ui.style()).inner_margin(PAD))
        // The reset confirmation is a second click inside the popup.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            // Enforce the content width: Popup::width only supplies an
            // initial size, and the popup can otherwise shrink to fit.
            ui.set_width(POPUP_W - 2.0 * PAD as f32);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.label(egui::RichText::new("KitPool").strong());
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label(format!("Build {}", env!("SAMPLE_MANAGER_BUILD")));
            ui.label(format!("Built {}", env!("SAMPLE_MANAGER_BUILD_DATE")));
            ui.separator();
            ui.hyperlink(env!("CARGO_PKG_REPOSITORY"));
            ui.hyperlink(env!("CARGO_PKG_HOMEPAGE"));
            ui.add_space(8.0);
            midi_ports(ui, &ports());
            ui.add_space(4.0);
            if reset_state(ui, confirm_id, can_reset) {
                out = Some(AboutIntent::ResetState);
            }
        });
    if shown.is_none() {
        // Closed: the next open starts on the plain button again.
        ui.data_mut(|d| d.remove::<bool>(confirm_id));
    }
    out
}

/// Inputs and outputs side by side, small, in a darker inset box so the
/// version and links above stay the popup's main content.
fn midi_ports(ui: &mut egui::Ui, p: &crate::device::MidiPorts) {
    let (used_in, used_out) = match &p.claimed {
        Some((i, o)) => (Some(i.as_str()), Some(o.as_str())),
        None => (None, None),
    };
    let list = |ui: &mut egui::Ui, title: &str, names: &[String], used: Option<&str>| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.label(egui::RichText::new(title).size(MIDI_TEXT).strong());
        ui.add_space(2.0);
        if names.is_empty() {
            ui.label(egui::RichText::new("none").size(MIDI_TEXT).weak());
        }
        for n in names {
            let text = if Some(n.as_str()) == used {
                egui::RichText::new(format!("{n} (using)")).color(super::theme::ACCENT_BRIGHT)
            } else {
                egui::RichText::new(n)
            };
            ui.label(text.size(MIDI_TEXT));
        }
    };
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(4)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.columns(2, |cols| {
                list(&mut cols[0], "MIDI inputs", &p.inputs, used_in);
                list(&mut cols[1], "MIDI outputs", &p.outputs, used_out);
            });
            if crate::device::switched_off() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Device switched off: ports aren't rescanned until it's back on.")
                        .size(MIDI_TEXT)
                        .weak(),
                );
            }
        });
}

/// Reset state, then a confirmation. True once confirmed.
fn reset_state(ui: &mut egui::Ui, confirm_id: egui::Id, can_reset: bool) -> bool {
    let confirming = ui.data(|d| d.get_temp::<bool>(confirm_id)).unwrap_or(false);
    if !confirming {
        let resp = ui
            .add_enabled(can_reset, egui::Button::new("Reset state…"))
            .on_hover_text(
                "Forget open tabs, favourites, file history marks, editor layout and open kits, \
                 then restart as on first run. Files on disk are not touched.",
            )
            .on_disabled_hover_text("Wait for the transfer to finish");
        if resp.clicked() {
            ui.data_mut(|d| d.insert_temp(confirm_id, true));
        }
        return false;
    }
    ui.label(
        "Restart with a fresh state? Open tabs, favourites, file history marks, editor layout and \
         open kits are forgotten (you're asked about unsaved edits first). This can't be undone.",
    );
    let mut go = false;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(can_reset, egui::Button::new("Reset and restart"))
            .clicked()
        {
            go = true;
        }
        if ui.button("Cancel").clicked() {
            ui.data_mut(|d| d.insert_temp(confirm_id, false));
        }
    });
    go
}
