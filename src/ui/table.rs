//! The stream table — the centerpiece. One row per output stream in output
//! order, with a drag handle, type badge, summary, flags, and an action.
//!
//! M3 is read-only: rows are selectable (drives the inspector) but the drag
//! handle is inert and every action reads "keep". Drag-reorder and the
//! convert/extract/remove actions land in M4.

use super::{InterlaceApp, badge, card};
use crate::model::{Encode, OutStream};

const ROW_HEIGHT: f32 = 26.0;
const COL_HANDLE: f32 = 18.0;
const COL_TYPE: f32 = 78.0;
const COL_FLAGS: f32 = 90.0;
const COL_ACTION: f32 = 64.0;

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let selected = app.selected;
    let mut clicked: Option<usize> = None;

    card(ui, |ui| {
        header_row(ui);
        ui.separator();

        let Some(project) = &app.project else {
            ui.add_space(6.0);
            ui.weak("Load a media file to see its streams.");
            ui.add_space(6.0);
            return;
        };

        for (i, stream) in project.streams.iter().enumerate() {
            if stream_row(ui, stream, selected == Some(i)) {
                clicked = Some(i);
            }
        }
    });

    if let Some(i) = clicked {
        app.selected = Some(i);
    }
}

fn header_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        cell(ui, COL_HANDLE, |_| {});
        cell(ui, COL_TYPE, |ui| head(ui, "TYPE"));
        // STREAM column takes the remaining width.
        let stream_w = (ui.available_width() - COL_FLAGS - COL_ACTION).max(80.0);
        cell(ui, stream_w, |ui| head(ui, "STREAM"));
        cell(ui, COL_FLAGS, |ui| head(ui, "FLAGS"));
        cell(ui, COL_ACTION, |ui| head(ui, "ACTION"));
    });
}

/// Draw one stream row. Returns true if it was clicked this frame.
fn stream_row(ui: &mut egui::Ui, s: &OutStream, is_selected: bool) -> bool {
    let fill = if is_selected {
        egui::Color32::from_rgb(30, 58, 138) // deep blue selection, per mockup
    } else {
        egui::Color32::TRANSPARENT
    };

    let inner = egui::Frame::NONE
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                cell(ui, COL_HANDLE, |ui| {
                    ui.weak("⋮⋮"); // drag handle (inert in M3)
                });
                cell(ui, COL_TYPE, |ui| badge(ui, s.source.kind));
                let stream_w = (ui.available_width() - COL_FLAGS - COL_ACTION).max(80.0);
                cell(ui, stream_w, |ui| {
                    ui.label(summary(s));
                });
                cell(ui, COL_FLAGS, |ui| {
                    ui.label(
                        egui::RichText::new(flags(s))
                            .color(egui::Color32::from_gray(170)),
                    );
                });
                cell(ui, COL_ACTION, |ui| {
                    // Everything is "keep" in M3; convert/extract/remove is M4.
                    ui.weak(action_text(s));
                });
            });
        });

    inner.response.interact(egui::Sense::click()).clicked()
}

/// A fixed-width cell laid out left-to-right and vertically centered, so columns
/// line up across the header and every row.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_height(ROW_HEIGHT);
            add(ui);
        },
    );
}

fn head(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .small()
            .strong()
            .color(egui::Color32::from_gray(140)),
    );
}

/// Stream summary: codec (with convert target if any), language, title, and the
/// source-input tag when it isn't the primary file. Mirrors the mockup, e.g.
/// `flac → aac · jpn · "Japanese"` or `subrip · eng · from 2`.
fn summary(s: &OutStream) -> String {
    let mut parts: Vec<String> = Vec::new();

    match &s.encode {
        Encode::Copy => parts.push(s.source.codec.clone()),
        Encode::Audio { codec, .. } => parts.push(format!("{} → {}", s.source.codec, codec)),
    }
    if let Some(lang) = &s.meta.language {
        parts.push(lang.clone());
    }
    if let Some(title) = &s.meta.title {
        parts.push(format!("\u{201C}{title}\u{201D}")); // “title”
    }
    if s.source.input != 0 {
        parts.push(format!("from {}", s.source.input));
    }
    parts.join(" · ")
}

fn flags(s: &OutStream) -> String {
    let mut f = Vec::new();
    if s.meta.default {
        f.push("default");
    }
    if s.meta.forced {
        f.push("forced");
    }
    f.join(" · ")
}

fn action_text(s: &OutStream) -> &'static str {
    match &s.encode {
        Encode::Copy => "keep",
        Encode::Audio { .. } => "convert",
    }
}
