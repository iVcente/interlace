//! The command card: the ffmpeg command line, in a resizable card under the
//! table, toggled by the terminal button in the sources card.
//!
//! It mirrors `Project::to_args()` live *until the user edits it* — then it
//! becomes an escape hatch: `InterlaceApp::command_edit` holds the edited text,
//! an amber "Edited" marker appears (and the box outline turns amber), and Run
//! executes that text verbatim (see `run::run_raw`). Reset drops back to the
//! live view. The validation hints under the table hide while it's edited,
//! since they describe the table, not the edited text.

use super::widgets::{self, section_label};
use super::{InterlaceApp, theme};
use egui::{Margin, RichText, Stroke, vec2};
use egui_phosphor::regular as icon;

/// The command card, filling the resizable bottom panel it's drawn into.
pub(super) fn show_command(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let edited = app.command_edit.is_some();
    // The edited buffer if the user has taken the escape hatch, otherwise the
    // live model command — recomputed each frame so it tracks edits made
    // elsewhere. When edited we don't build the model command at all: it isn't
    // shown, and `to_args()` allocates the whole arg vector every frame.
    let mut text = match &app.command_edit {
        Some(edited) => edited.clone(),
        None => match &app.project {
            // Quoted so an edit round-trips through `run::tokenize`.
            Some(p) => format!("ffmpeg {}", crate::run::join_args(&p.to_args())),
            None => String::new(),
        },
    };
    let mut reset = false;
    let mut copy = false;
    let height = ui.available_height();

    widgets::card_fill(ui, height, Margin::symmetric(10, 8), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            ui.set_min_height(24.0);
            section_label(ui, "COMMAND");
            if edited {
                ui.add_space(4.0);
                let (dot, _) = ui.allocate_exact_size(vec2(7.0, 7.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.5, theme::WARNING);
                widgets::line(ui, RichText::new("Edited. No longer matches the table.").size(12.0).color(theme::WARNING));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().button_padding = vec2(8.0, 2.0);
                let small = |glyph, label| widgets::button(RichText::new(widgets::with_icon(glyph, label)).size(12.0));
                copy = ui.add(small(icon::COPY, "Copy")).on_hover_text("Copy the command").clicked();
                if edited {
                    reset = ui
                        .add(small(icon::ARROW_COUNTER_CLOCKWISE, "Reset"))
                        .on_hover_text("Go back to the command the table builds")
                        .clicked();
                }
            });
        });

        let stroke = if edited { theme::WARNING } else { theme::STROKE };
        egui::Frame::NONE
            .fill(theme::FIELD)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(6)
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                let size = ui.available_size();
                egui::ScrollArea::vertical()
                    .id_salt("command_scroll")
                    .auto_shrink([false, false])
                    .max_height(size.y)
                    .show(ui, |ui| {
                        let response = ui.add_sized(
                            vec2(size.x, (size.y - 2.0).max(20.0)),
                            egui::TextEdit::multiline(&mut text)
                                .frame(egui::Frame::NONE)
                                .font(egui::FontId::monospace(12.5))
                                .text_color(theme::CODE)
                                .margin(Margin::ZERO),
                        );
                        if response.changed() {
                            // First keystroke takes the escape hatch; further edits update it.
                            app.command_edit = Some(text.clone());
                        }
                    });
            });
    });

    if copy {
        ui.ctx().copy_text(text);
    }
    if reset {
        app.command_edit = None;
    }
}
