//! The stream table — the centerpiece. One row per output stream in output
//! order, with a drag handle, type badge, summary, flags, and actions.
//!
//! Rows are drag-to-reorder (via egui's built-in `dnd_drag_source` /
//! `dnd_drop_zone` — only the handle initiates a drag, so the row body stays
//! clickable for selection) and carry remove / convert actions. Mutations are
//! collected during the immutable render pass and applied to `app` afterwards.

use super::{InterlaceApp, badge, card};
use crate::model::{Encode, OutStream};

const ROW_HEIGHT: f32 = 26.0;
const COL_HANDLE: f32 = 20.0;
const COL_TYPE: f32 = 78.0;
const COL_FLAGS: f32 = 110.0;
const COL_ACTION: f32 = 116.0;

enum RowAction {
    Remove,
    ToggleConvert,
}

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let selected = app.selected;
    let mut click: Option<usize> = None;
    let mut action: Option<(usize, RowAction)> = None;
    let mut reorder: Option<(usize, usize)> = None;

    card(ui, |ui| {
        header_row(ui);
        ui.separator();

        let Some(project) = &app.project else {
            ui.add_space(6.0);
            ui.weak("Load a media file to see its streams.");
            ui.add_space(6.0);
            return;
        };

        // The whole list is a drop zone; each row's handle is a drag source.
        let mut rects: Vec<egui::Rect> = Vec::with_capacity(project.streams.len());
        let (_, dropped) = ui.dnd_drop_zone::<usize, ()>(egui::Frame::NONE, |ui| {
            for (i, stream) in project.streams.iter().enumerate() {
                let rect = stream_row(ui, stream, i, selected == Some(i), &mut click, &mut action);
                rects.push(rect);
            }
        });

        // Where would a drop land? Count rows whose center is above the pointer.
        let pointer = ui.ctx().pointer_interact_pos();
        let dragging = egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx());
        if let Some(p) = pointer {
            if let Some(payload) = &dropped {
                let to = rects.iter().filter(|r| p.y > r.center().y).count();
                reorder = Some((**payload, to));
            } else if dragging && !rects.is_empty() {
                let to = rects.iter().filter(|r| p.y > r.center().y).count();
                draw_insertion_line(ui, &rects, to);
            }
        }
    });

    // Apply mutations after the immutable borrow of `app.project` ends. An action
    // takes precedence over a plain selection click on the same row.
    if let Some((i, act)) = action {
        match act {
            RowAction::Remove => app.remove_stream(i),
            RowAction::ToggleConvert => app.toggle_convert(i),
        }
    } else if let Some(i) = click {
        app.selected = Some(i);
    }
    if let Some((from, to)) = reorder {
        app.reorder(from, to);
    }
}

fn header_row(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        // Cells carry their own fixed widths; zero the gap between them so the
        // widths sum exactly to the row and columns don't drift or wrap.
        ui.spacing_mut().item_spacing.x = 0.0;
        cell(ui, COL_HANDLE, |_| {});
        cell(ui, COL_TYPE, |ui| head(ui, "TYPE"));
        let stream_w = (ui.available_width() - COL_FLAGS - COL_ACTION).max(80.0);
        cell(ui, stream_w, |ui| head(ui, "STREAM"));
        cell(ui, COL_FLAGS, |ui| head(ui, "FLAGS"));
        cell_rtl(ui, COL_ACTION, |ui| head(ui, "ACTION"));
    });
}

/// Draw one stream row; returns its rect. Sets `click`/`action` out-params.
fn stream_row(
    ui: &mut egui::Ui,
    s: &OutStream,
    i: usize,
    is_selected: bool,
    click: &mut Option<usize>,
    action: &mut Option<(usize, RowAction)>,
) -> egui::Rect {
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
                // Match header_row: fixed-width cells, no inter-cell gap.
                ui.spacing_mut().item_spacing.x = 0.0;
                cell(ui, COL_HANDLE, |ui| {
                    // Only the handle starts a drag; payload is the row index.
                    ui.dnd_drag_source(egui::Id::new("stream_dnd").with(i), i, |ui| {
                        ui.label(egui::RichText::new("⠿").weak());
                    });
                });
                cell(ui, COL_TYPE, |ui| badge(ui, s.source.kind));
                let stream_w = (ui.available_width() - COL_FLAGS - COL_ACTION).max(80.0);
                cell(ui, stream_w, |ui| {
                    // Truncate to the cell so a long title can't overflow and
                    // shove the FLAGS/ACTION columns out of alignment; the full
                    // text stays available on hover.
                    let text = summary(s);
                    ui.add(egui::Label::new(text.as_str()).truncate())
                        .on_hover_text(text);
                });
                cell(ui, COL_FLAGS, |ui| {
                    let text = egui::RichText::new(flags(s)).color(egui::Color32::from_gray(170));
                    ui.add(egui::Label::new(text).truncate());
                });
                cell_rtl(ui, COL_ACTION, |ui| {
                    // Rightmost first in a right-to-left layout.
                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                        *action = Some((i, RowAction::Remove));
                    }
                    if matches!(s.source.kind, crate::model::Kind::Audio) {
                        let (label, hover) = match s.encode {
                            Encode::Copy => ("convert", "Convert this audio (re-encode)"),
                            Encode::Audio { .. } => ("keep", "Revert to stream-copy"),
                        };
                        if ui.small_button(label).on_hover_text(hover).clicked() {
                            *action = Some((i, RowAction::ToggleConvert));
                        }
                    }
                });
            });
        });

    // The row body is clickable for selection (buttons above consume their own).
    if inner.response.interact(egui::Sense::click()).clicked() {
        *click = Some(i);
    }
    inner.response.rect
}

/// A fixed-width, vertically-centered cell so columns line up across rows.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    lay(ui, width, egui::Layout::left_to_right(egui::Align::Center), add);
}

/// A right-aligned fixed-width cell (for the ACTION column).
fn cell_rtl(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    lay(ui, width, egui::Layout::right_to_left(egui::Align::Center), add);
}

fn lay(ui: &mut egui::Ui, width: f32, layout: egui::Layout, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(egui::vec2(width, ROW_HEIGHT), layout, |ui| {
        // Pin the cell to *exactly* `width`. `allocate_ui_with_layout` otherwise
        // shrinks a cell to its content (and leaves the wrap width unbounded), so
        // without this columns collapse together and long labels shove later
        // columns sideways instead of truncating.
        ui.set_min_width(width);
        ui.set_max_width(width);
        ui.set_min_height(ROW_HEIGHT);
        add(ui);
    });
}

fn head(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .small()
            .strong()
            .color(egui::Color32::from_gray(140)),
    );
}

fn draw_insertion_line(ui: &egui::Ui, rects: &[egui::Rect], to: usize) {
    let y = if to == 0 {
        rects[0].top()
    } else {
        rects[to.min(rects.len()) - 1].bottom()
    };
    let x = ui.min_rect().x_range();
    ui.painter()
        .hline(x, y, egui::Stroke::new(2.0, egui::Color32::from_rgb(90, 140, 255)));
}

/// Stream summary, mirroring the mockup: `flac → aac · jpn · "Japanese"` or
/// `subrip · eng · from 2`.
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
        parts.push(format!("\u{201C}{title}\u{201D}"));
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
