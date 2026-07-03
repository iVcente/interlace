//! The stream table — the centerpiece. One row per output stream in output
//! order, with a drag handle, type badge, summary, flags, and a change summary.
//!
//! Rows are drag-to-reorder (via egui's built-in `dnd_drag_source` /
//! `dnd_drop_zone` — only the handle initiates a drag, so the row body stays
//! clickable for selection). Editing happens in the side-panel inspector; this
//! table only *shows* state, so the ACTION column is now a set of read-only
//! badges describing how each row differs from the original (`convert`, `retag`,
//! `flags`, or a red `remove` for a stream marked for removal).

use super::{InterlaceApp, badge, card};
use crate::model::{Encode, Input, OutStream};

const ROW_HEIGHT: f32 = 26.0;
const COL_HANDLE: f32 = 20.0;
const COL_TYPE: f32 = 78.0;
const COL_FLAGS: f32 = 90.0;
const COL_ACTION: f32 = 168.0;

// Change-badge colors.
const C_REMOVE: egui::Color32 = egui::Color32::from_rgb(220, 80, 80);
const C_CONVERT: egui::Color32 = egui::Color32::from_rgb(124, 58, 237);
const C_RETAG: egui::Color32 = egui::Color32::from_rgb(217, 119, 6);
const C_FLAGS: egui::Color32 = egui::Color32::from_rgb(37, 99, 235);
const C_EMBED: egui::Color32 = egui::Color32::from_rgb(46, 160, 90); // green
const C_OFFSET: egui::Color32 = egui::Color32::from_rgb(13, 148, 136); // teal

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let selected = app.selected;
    let mut click: Option<usize> = None;
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
                // Every stream references a valid input (the model invariant), so
                // its source input is always present.
                let input = &project.inputs[stream.source.input];
                let rect = stream_row(ui, stream, input, i, selected == Some(i), &mut click);
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

    // Apply mutations after the immutable borrow of `app.project` ends.
    if let Some(i) = click {
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
        cell(ui, COL_ACTION, |ui| head(ui, "CHANGES"));
    });
}

/// Draw one stream row; returns its rect. Sets `click` when the row body (not the
/// drag handle) is clicked, so the inspector can select it.
fn stream_row(
    ui: &mut egui::Ui,
    s: &OutStream,
    input: &Input,
    i: usize,
    is_selected: bool,
    click: &mut Option<usize>,
) -> egui::Rect {
    let fill = if is_selected {
        egui::Color32::from_rgb(30, 58, 138) // deep blue selection, per mockup
    } else if s.removed {
        egui::Color32::from_rgb(58, 28, 28) // faint red wash for a removed row
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
                        drag_handle(ui);
                    });
                });
                cell(ui, COL_TYPE, |ui| badge(ui, s.source.kind));
                let stream_w = (ui.available_width() - COL_FLAGS - COL_ACTION).max(80.0);
                cell(ui, stream_w, |ui| {
                    // Truncate to the cell so a long title can't overflow and
                    // shove the FLAGS/CHANGES columns out of alignment. A truncated
                    // Label surfaces the full text on hover on its own, so we add
                    // no extra tooltip (that would double it up, and would show even
                    // when nothing is elided). Removed rows are struck out.
                    let text = summary(s, input);
                    let rich = if s.removed {
                        egui::RichText::new(&text)
                            .strikethrough()
                            .color(egui::Color32::from_gray(130))
                    } else {
                        egui::RichText::new(&text)
                    };
                    ui.add(egui::Label::new(rich).truncate());
                });
                cell(ui, COL_FLAGS, |ui| {
                    let text = egui::RichText::new(flags(s)).color(egui::Color32::from_gray(170));
                    ui.add(egui::Label::new(text).truncate());
                });
                cell(ui, COL_ACTION, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for b in changes(s, input) {
                        pill(ui, b.label, b.color, &b.hover);
                    }
                });
            });
        });

    // The row body is clickable for selection.
    if inner.response.interact(egui::Sense::click()).clicked() {
        *click = Some(i);
    }
    inner.response.rect
}

/// Paint a 2×3 dot grip as the drag handle. Drawn with the painter rather than a
/// glyph so it doesn't depend on font coverage (the old "⠿" braille char rendered
/// as a missing-glyph box in egui's bundled fonts).
fn drag_handle(ui: &mut egui::Ui) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(10.0, ROW_HEIGHT), egui::Sense::hover());
    let color = if resp.hovered() {
        egui::Color32::from_gray(180)
    } else {
        egui::Color32::from_gray(110)
    };
    let c = rect.center();
    let painter = ui.painter();
    for dx in [-2.5, 2.5] {
        for dy in [-4.0, 0.0, 4.0] {
            painter.circle_filled(egui::pos2(c.x + dx, c.y + dy), 1.1, color);
        }
    }
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

/// A fixed-width, vertically-centered cell so columns line up across rows.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // Pin the cell to *exactly* `width`. `allocate_ui_with_layout` otherwise
            // shrinks a cell to its content (and leaves the wrap width unbounded), so
            // without this columns collapse together and long labels shove later
            // columns sideways instead of truncating.
            ui.set_min_width(width);
            ui.set_max_width(width);
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

/// A small filled, rounded badge with a hover tooltip — the change indicators.
fn pill(ui: &mut egui::Ui, text: &str, color: egui::Color32, hover: &str) {
    let resp = egui::Frame::NONE
        .fill(color)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .small()
                    .strong()
                    .color(egui::Color32::WHITE),
            );
        })
        .response;
    if !hover.is_empty() {
        resp.on_hover_text(hover);
    }
}

/// One change indicator for the CHANGES column.
struct Change {
    label: &'static str,
    color: egui::Color32,
    hover: String,
}

/// The badges describing how `s` differs from its probed original. A removed
/// stream shows only the red `remove` badge; otherwise any of `embed` / `convert`
/// / `retag` / `flags` / `offset` that apply (empty when the stream is untouched).
/// `input` is the stream's source input, needed for the `embed`/`offset` badges.
fn changes(s: &OutStream, input: &Input) -> Vec<Change> {
    if s.removed {
        return vec![Change {
            label: "remove",
            color: C_REMOVE,
            hover: "Will be dropped from the output".into(),
        }];
    }

    let mut out = Vec::new();
    // An embedded external track: synthetic *and* drawn from an added input (a
    // converted-copy sibling is also synthetic but stays on the primary input).
    if s.added && s.source.input != 0 {
        out.push(Change {
            label: "embed",
            color: C_EMBED,
            hover: "Embedded from an added file".into(),
        });
    }
    if s.converted() {
        let to = match &s.encode {
            Encode::Audio { codec, .. } => codec.as_str(),
            Encode::Copy => "copy",
        };
        out.push(Change {
            label: "convert",
            color: C_CONVERT,
            hover: format!("re-encode {} » {}", s.source.codec, to),
        });
    }
    if s.tags_changed() {
        out.push(Change {
            label: "retag",
            color: C_RETAG,
            hover: tag_diff(s),
        });
    }
    if s.flags_changed() {
        out.push(Change {
            label: "flags",
            color: C_FLAGS,
            hover: format!(
                "default {} » {}, forced {} » {}",
                on_off(s.orig_meta.default),
                on_off(s.meta.default),
                on_off(s.orig_meta.forced),
                on_off(s.meta.forced),
            ),
        });
    }
    // A sync offset on the source input (gated on integer ms, like `to_args`).
    if (input.offset_secs * 1000.0).round() as i64 != 0 {
        out.push(Change {
            label: "offset",
            color: C_OFFSET,
            hover: format!("timestamps shifted {:+.3} s", input.offset_secs),
        });
    }
    out
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// A human summary of the language/title edits for the `retag` badge tooltip.
fn tag_diff(s: &OutStream) -> String {
    let mut parts = Vec::new();
    if s.meta.language != s.orig_meta.language {
        parts.push(format!(
            "language: {} » {}",
            opt(&s.orig_meta.language),
            opt(&s.meta.language)
        ));
    }
    if s.meta.title != s.orig_meta.title {
        parts.push(format!(
            "title: {} » {}",
            opt(&s.orig_meta.title),
            opt(&s.meta.title)
        ));
    }
    parts.join("\n")
}

fn opt(v: &Option<String>) -> String {
    match v {
        Some(s) => format!("\u{201C}{s}\u{201D}"),
        None => "—".into(),
    }
}

/// Stream summary, mirroring the mockup: `flac » aac · jpn · "Japanese"` or, for
/// an embedded track, `subrip · eng · from subs.srt · +0.15s`.
fn summary(s: &OutStream, input: &Input) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &s.encode {
        Encode::Copy => parts.push(s.source.codec.clone()),
        Encode::Audio { codec, .. } => parts.push(format!("{} » {}", s.source.codec, codec)),
    }
    if let Some(lang) = &s.meta.language {
        parts.push(lang.clone());
    }
    if let Some(title) = &s.meta.title {
        parts.push(format!("\u{201C}{title}\u{201D}"));
    }
    // Embedded tracks come from a non-primary input: name the file it came from,
    // and its sync offset if one is set.
    if s.source.input != 0 {
        let name = input
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("input {}", s.source.input));
        parts.push(format!("from {name}"));
        if (input.offset_secs * 1000.0).round() as i64 != 0 {
            parts.push(format!("{:+}s", trim_offset(input.offset_secs)));
        }
    }
    parts.join(" · ")
}

/// The offset in seconds with trailing-zero noise trimmed, for a compact summary
/// (`0.15` not `0.150`). Rounded to the millisecond the command actually uses.
fn trim_offset(secs: f64) -> f64 {
    ((secs * 1000.0).round()) / 1000.0
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Meta, Source};
    use std::path::PathBuf;

    fn out(input: usize, added: bool, meta: Meta) -> OutStream {
        let mut s = OutStream::new(
            Source { input, index: 0, kind: Kind::Subtitle, codec: "subrip".into(), bitrate_kbps: None },
            meta,
            Encode::Copy,
        );
        s.added = added;
        s
    }

    fn input(name: &str, offset_secs: f64) -> Input {
        Input { path: PathBuf::from(name), offset_secs }
    }

    fn labels(cs: &[Change]) -> Vec<&str> {
        cs.iter().map(|c| c.label).collect()
    }

    #[test]
    fn embedded_summary_names_file_and_offset() {
        let s = out(1, true, Meta { language: Some("eng".into()), ..Default::default() });
        let text = summary(&s, &input("/media/subs.srt", 0.15));
        assert_eq!(text, "subrip · eng · from subs.srt · +0.15s");
    }

    #[test]
    fn embedded_summary_without_offset_omits_it() {
        let s = out(1, true, Meta::default());
        let text = summary(&s, &input("subs.srt", 0.0));
        assert_eq!(text, "subrip · from subs.srt");
    }

    #[test]
    fn embed_and_offset_badges_on_embedded_track() {
        let s = out(1, true, Meta::default());
        assert_eq!(labels(&changes(&s, &input("subs.srt", -0.2))), ["embed", "offset"]);
    }

    #[test]
    fn no_embed_badge_for_primary_input_or_converted_sibling() {
        // A synthetic convert-sibling stays on input 0 → not an embed.
        let sibling = out(0, true, Meta::default());
        assert!(!labels(&changes(&sibling, &input("in.mkv", 0.0))).contains(&"embed"));
        // A plain probed primary stream → no badges at all.
        let primary = out(0, false, Meta::default());
        assert!(changes(&primary, &input("in.mkv", 0.0)).is_empty());
    }
}
