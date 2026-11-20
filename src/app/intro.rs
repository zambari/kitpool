//! First-run intro for the editor's **Show** row. On first run every panel is
//! shown; once the first sample has loaded (so the panels have something in
//! them) and [`DELAY`] has passed, the extra panels switch off one by one,
//! each chip pinging as it goes, so a new user sees that those chips hide and show
//! parts of the editor. It ends on [`Panels::default`], which stays the one
//! source of truth for the resting layout.

use super::*;
use crate::editor::Panels;
use std::time::{Duration, Instant};

const DELAY: Duration = Duration::from_secs(2);
/// From the first switch-off to the last.
const SPAN: Duration = Duration::from_millis(1000);

/// Show-row chip label (as drawn by `panel_toggles`) and its flag, in the
/// order they switch off.
const STEPS: [(&str, fn(&mut Panels) -> &mut bool); 5] = [
    ("Nums", |p| &mut p.nums),
    ("Histogram", |p| &mut p.histogram),
    ("Help", |p| &mut p.help),
    ("Transport", |p| &mut p.transport),
    ("Tools", |p| &mut p.tools),
];

pub(super) struct PanelIntro {
    /// When the first sample showed up; `None` until then.
    start: Option<Instant>,
    /// Steps already applied.
    done: usize,
    /// What the panels should be now; anything else means the user clicked
    /// a chip, and the intro stops there.
    expect: Panels,
}

impl PanelIntro {
    /// Starts from [`Panels::all_on`]; the caller shows that.
    pub(super) fn new() -> Self {
        debug_assert!(
            {
                let mut p = Panels::all_on();
                for (_, flag) in STEPS {
                    *flag(&mut p) = false;
                }
                p == Panels::default()
            },
            "the intro's steps must end on Panels::default()"
        );
        Self {
            start: None,
            done: 0,
            expect: Panels::all_on(),
        }
    }

    fn due(start: Instant, i: usize) -> Instant {
        let gap = SPAN / (STEPS.len() as u32 - 1);
        start + DELAY + gap * i as u32
    }
}

impl App {
    /// Call once a frame. Applies the steps that are due to the front
    /// editor's panels (prefs travel with the front view on slot swaps).
    pub(super) fn tick_intro(&mut self, ctx: &egui::Context) {
        let Some(intro) = self.panel_intro.as_mut() else {
            return;
        };
        let panels = &mut self.editor.prefs.panels;
        if *panels != intro.expect {
            // The user found the chips on their own: leave their choice.
            self.panel_intro = None;
            return;
        }
        let now = Instant::now();
        let start = match intro.start {
            Some(t) => t,
            // Waiting for the first sample to load.
            None if self.editor.current.is_none() => return,
            None => *intro.start.insert(now),
        };
        while intro.done < STEPS.len() && now >= PanelIntro::due(start, intro.done) {
            let (label, flag) = STEPS[intro.done];
            *flag(panels) = false;
            editor_view::ping_panel_chip(ctx, label);
            intro.done += 1;
        }
        intro.expect = panels.clone();
        if intro.done == STEPS.len() {
            self.panel_intro = None;
        } else {
            ctx.request_repaint_after(PanelIntro::due(start, intro.done).saturating_duration_since(now));
        }
    }

    /// Panels to save: while the intro runs, where it will end, so quitting
    /// mid-way doesn't keep every panel on.
    pub(super) fn panels_to_save(&self) -> Panels {
        if self.panel_intro.is_some() {
            Panels::default()
        } else {
            self.editor.prefs.panels.clone()
        }
    }
}
