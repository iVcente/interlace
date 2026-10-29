//! The sources card at the top: one chip per loaded input, the `+ Audio` /
//! `+ Subtitle` buttons, the command-panel and settings toggles, and (once a
//! file is open) the container TITLE field on a second row.
//!
//! `+ Audio` and `+ Subtitle` insert an external file of that kind as an
//! additional input, embedding its single track into the current project (see
//! `InterlaceApp::embed_file`); they're disabled until a file is open. There's
//! no button for opening the video itself: that's the empty table's
//! `Add file…`, or a drop, which also covers replacing it.
//!
//! The buttons are laid out first, from the right, and the chips get whatever
//! width is left in a horizontal scroll strip. So a long file name can only
//! truncate its chip; it can never push a button off the row or widen the window.

use super::widgets;
use super::{InterlaceApp, pick_audio_file, pick_subtitle_file, theme};
use egui::{CornerRadius, Margin, RichText, Stroke, vec2};
use egui_phosphor::regular as icon;

/// Label column width for the TITLE row.
const LABEL_WIDTH: f32 = 60.0;

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp, wide: bool) {
    let mut audio_requested = false;
    let mut subtitle_requested = false;
    let mut toggle_settings = false;
    let mut toggle_command = false;
    let mut remove_input: Option<usize> = None;
    let busy = app.run_state.is_running();

    widgets::card(ui, Margin::symmetric(12, 8), |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            ui.add_sized(vec2(LABEL_WIDTH - 6.0, 20.0), egui::Label::new(widgets::caps("SOURCES", theme::WEAK)).selectable(false));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let size = vec2(28.0, 26.0);
                if widgets::icon_button(ui, icon::GEAR_SIX, size, app.show_settings, "Settings").clicked() {
                    toggle_settings = true;
                }
                if widgets::icon_button(ui, icon::TERMINAL_WINDOW, size, app.show_command, "Show the ffmpeg command")
                    .clicked()
                {
                    toggle_command = true;
                }
                // A 1×18 divider between the toggles and the add buttons.
                let (rect, _) = ui.allocate_exact_size(vec2(1.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, theme::STROKE);

                // Laid out right to left, so `+ Subtitle` goes in first.
                let can_insert = app.project.is_some() && !busy;
                subtitle_requested = ui
                    .add_enabled(can_insert, widgets::button(widgets::with_icon(icon::PLUS, "Subtitle")))
                    .on_hover_text("Insert a subtitle file as a track")
                    .clicked();
                audio_requested = ui
                    .add_enabled(can_insert, widgets::button(widgets::with_icon(icon::PLUS, "Audio")))
                    .on_hover_text("Insert an audio file as a track")
                    .clicked();

                // The chips take the width left of the buttons.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    match &app.project {
                        Some(project) => {
                            egui::ScrollArea::horizontal()
                                .id_salt("source_chips")
                                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        let max = if wide { 420.0 } else { 250.0 };
                                        for (k, input) in project.inputs.iter().enumerate() {
                                            let name = super::file_name_lossy(&input.path)
                                                .unwrap_or_else(|| input.path.display().to_string());
                                            if chip(ui, k, &name, max, !busy) {
                                                remove_input = Some(k);
                                            }
                                        }
                                    });
                                });
                        }
                        None => {
                            ui.label("No input yet");
                        }
                    }
                });
            });
        });

        // The container title (whole-file metadata). Empty clears it back to
        // `None`, so an untouched title round-trips through the copy unchanged.
        if let Some(project) = app.project.as_mut() {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(LABEL_WIDTH - 6.0, 20.0), egui::Label::new(widgets::caps("TITLE", theme::WEAK)).selectable(false));
                let mut title = project.title.clone().unwrap_or_default();
                let resp = ui
                    .add(
                        egui::TextEdit::singleline(&mut title)
                            .hint_text("File title")
                            .margin(Margin::symmetric(8, 4))
                            .desired_width(f32::INFINITY),
                    )
                    .on_hover_text("Sets the output container's title metadata");
                if resp.changed() {
                    project.title = (!title.is_empty()).then_some(title);
                }
            });
        }
    });

    if toggle_settings {
        app.show_settings = !app.show_settings;
    }
    if toggle_command {
        app.show_command = !app.show_command;
    }
    if let Some(k) = remove_input {
        app.remove_input(k);
    }
    // Run the (blocking) native dialogs after the borrow of `app` ends.
    if audio_requested && let Some(path) = pick_audio_file() {
        app.embed_file(path);
    }
    if subtitle_requested && let Some(path) = pick_subtitle_file() {
        app.embed_file(path);
    }
}

/// One source chip: the input index badge, the file name (truncated to
/// `max_width`), and an `x` that removes the input. Returns whether the `x` was
/// clicked.
fn chip(ui: &mut egui::Ui, index: usize, name: &str, max_width: f32, removable: bool) -> bool {
    let mut remove = false;
    egui::Frame::NONE
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::STROKE))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin { left: 4, right: 6, top: 3, bottom: 3 })
        .show(ui, |ui| {
            ui.set_max_width(max_width);
            ui.spacing_mut().item_spacing.x = 6.0;
            egui::Frame::NONE
                .fill(theme::BUTTON_STROKE)
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(5, 1))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(index.to_string()).font(egui::FontId::monospace(11.0)))
                            .selectable(false),
                    );
                });
            // The name gets its natural width up to what the cap leaves after the
            // badge and the `x`; past that it truncates.
            let natural = ui
                .painter()
                .layout_no_wrap(name.to_string(), egui::FontId::proportional(13.0), theme::TEXT)
                .size()
                .x;
            let name_width = natural.min(max_width - 56.0).max(24.0);
            ui.allocate_ui_with_layout(
                vec2(name_width, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_max_width(name_width);
                    widgets::line(ui, name);
                },
            );
            let hover = if index == 0 { "Close this file" } else { "Remove this track file and its streams" };
            ui.add_enabled_ui(removable, |ui| {
                remove = widgets::close_x(ui, 16.0, hover);
            });
        });
    remove
}
