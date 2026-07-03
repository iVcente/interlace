//! The top bar: one chip per loaded input file, plus the "add file" and
//! settings buttons on the right.
//!
//! M3 loads a single primary file, shown as a chip. Appending *additional*
//! inputs (true stream insertion) is M4, so for now "add file" opens a picker
//! and loads it as the primary project.

use super::{InterlaceApp, card, pick_media_file};

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut open_requested = false;
    let mut toggle_settings = false;

    card(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(project) = &app.project {
                for path in &project.inputs {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    chip(ui, &name);
                }
            }
            // Add-file and settings sit together on the right of the same row.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙").on_hover_text("Binaries / settings").clicked() {
                    toggle_settings = true;
                }
                if ui.button("+ add file").clicked() {
                    open_requested = true;
                }
            });
        });
    });

    if toggle_settings {
        app.show_settings = !app.show_settings;
    }
    // Run the (blocking) native dialog after the borrow of `app` ends.
    if open_requested && let Some(path) = pick_media_file() {
        app.load_file(path);
    }
}

/// A read-only rounded chip showing a source file.
fn chip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 4))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.label(text);
        });
}
