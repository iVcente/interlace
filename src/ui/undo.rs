//! Undo for the coarse source changes: a drop (open, replace, insert) or a
//! source chip's ×. Each pushes a snapshot of the session before it ran, and an
//! Undo toast over the run card offers it back; Ctrl+Z takes the newest.
//!
//! Snapshots are whole `Project` clones: a project is a few dozen small structs,
//! and a snapshot can't drift out of sync with the model the way a hand-written
//! inverse operation could. Field edits in the inspector aren't recorded; they
//! are already visible and reversible where they're made.

use super::selection::Selection;
use super::widgets;
use super::{InterlaceApp, theme};
use crate::model::Project;
use egui::{Margin, RichText, Stroke, vec2};
use egui_phosphor::regular as icon;
use std::time::{Duration, Instant};

/// How long the toast stays up.
const TOAST_FOR: Duration = Duration::from_secs(6);
/// How many steps are kept.
const MAX_STEPS: usize = 20;

/// The session as it was before one source change.
pub(crate) struct Snapshot {
    project: Option<Project>,
    selection: Selection,
    command_edit: Option<String>,
}

/// The toast announcing the last source change.
pub(crate) struct Toast {
    /// What happened, e.g. `Added S1 from subs.srt`.
    pub(crate) text: String,
    /// When it closes by itself.
    expires: Instant,
}

impl InterlaceApp {
    /// The session as it is now, to hand to `push_undo` after a change.
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            project: self.project.clone(),
            selection: self.selection.clone(),
            command_edit: self.command_edit.clone(),
        }
    }

    /// Record `before` as an undo step and announce `text` in the toast.
    pub(crate) fn push_undo(&mut self, before: Snapshot, text: String) {
        self.undo.push(before);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.toast = Some(Toast { text, expires: Instant::now() + TOAST_FOR });
    }

    /// Go back one step. Ignored while a job runs, since it captured the project.
    pub(crate) fn undo(&mut self) {
        if self.run_state.is_running() {
            return;
        }
        let Some(before) = self.undo.pop() else { return };
        self.project = before.project;
        self.selection = before.selection;
        self.command_edit = before.command_edit;
        self.toast = None;
    }
}

/// Ctrl+Z, unless a text field has focus (where it undoes typing instead).
pub(super) fn shortcut(ctx: &egui::Context, app: &mut InterlaceApp) {
    if app.undo.is_empty() || ctx.egui_wants_keyboard_input() {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
        app.undo();
    }
}

/// The toast, right-aligned 8 px above `bottom` (the run card's top edge).
pub(super) fn toast(ctx: &egui::Context, app: &mut InterlaceApp, bottom: f32) {
    let Some(toast) = &app.toast else { return };
    let left = toast.expires.saturating_duration_since(Instant::now());
    if left.is_zero() {
        app.toast = None;
        return;
    }
    ctx.request_repaint_after(left);

    let right = ctx.content_rect().right() - theme::GUTTER;
    let (mut undo, mut close) = (false, false);
    egui::Area::new(egui::Id::new("undo_toast"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(egui::pos2(right, bottom - theme::GUTTER))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::CARD)
                .stroke(Stroke::new(1.0, theme::BUTTON_STROKE))
                .corner_radius(8)
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_max_width(520.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(RichText::new(icon::CHECK_CIRCLE).size(15.0).color(theme::SUCCESS));
                        let budget = (520.0 - 150.0_f32).max(80.0);
                        ui.allocate_ui(vec2(budget, 20.0), |ui| widgets::line(ui, toast.text.as_str()));
                        undo = ui
                            .add(
                                egui::Button::new(RichText::new("Undo").font(theme::medium_font(13.0)).color(theme::ACCENT))
                                    .frame(false),
                            )
                            .clicked();
                        ui.label(RichText::new("Ctrl+Z").font(egui::FontId::monospace(11.0)).color(theme::WEAK));
                        close = widgets::close_x(ui, 18.0, "Dismiss");
                    });
                });
        });
    if undo {
        app.undo();
    } else if close {
        app.toast = None;
    }
}
