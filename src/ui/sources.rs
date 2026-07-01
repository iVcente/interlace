//! The sources bar: one chip per loaded input file, plus an "add file" button.
//!
//! M3 loads a single primary file; the chip's index prefix matches the ffmpeg
//! `-i` index. Appending *additional* inputs (true stream insertion) is M4, so
//! for now "add file" opens a picker and loads it as the primary project.

use super::{InterlaceApp, card, pick_media_file, section_label};

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut open_requested = false;

    card(ui, |ui| {
        section_label(ui, "SOURCES");
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(project) = &app.project {
                for (i, path) in project.inputs.iter().enumerate() {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    chip(ui, &format!("{i} · {name}"));
                }
            }
            if ui.button("+ add file").clicked() {
                open_requested = true;
            }
        });
    });

    // Run the (blocking) native dialog after the borrow of `app` ends.
    if open_requested {
        if let Some(path) = pick_media_file() {
            app.load_file(path);
        }
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
