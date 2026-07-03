//! The inspector: the selected row's fields as an editable form, shown in the
//! right-hand side panel. Edits write straight back into
//! `project.streams[selected]` — the table row and inspector are the same object
//! shown two ways.
//!
//! `language`/`title` map an empty field to `None` (inherit the original tag
//! through the copy). The convert controls switch a stream between `Encode::Copy`
//! and `Encode::Audio` and tune its codec/bitrate/channels. The Remove button is
//! *soft*: it sets `stream.removed` (the row stays, dimmed, and `to_args()` skips
//! it) so the change is visible in the table and reversible via Restore.

use super::{InterlaceApp, card, section_label};
use crate::model::{BITRATE_LADDER, Bitrate, Encode, Kind, OutStream};

const CODECS: [&str; 5] = ["aac", "ac3", "opus", "flac", "mp3"];
const CHANNELS: [(Option<u32>, &str); 4] = [
    (None, "source"),
    (Some(1), "1 (mono)"),
    (Some(2), "2 (stereo)"),
    (Some(6), "6 (5.1)"),
];
const RED: egui::Color32 = egui::Color32::from_rgb(220, 80, 80);
const GREEN: egui::Color32 = egui::Color32::from_rgb(46, 160, 90);

/// A fixed 22×22 close button whose `×` is painted at the exact rect center
/// (egui's `Button` can't perfectly center a lone glyph). The fixed footprint
/// also stops hover expansion from reflowing the header. Returns `true` on click.
fn close_button(ui: &mut egui::Ui) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let resp = resp.on_hover_text("Close inspector");
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(4), v.weak_bg_fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(16.0),
            v.fg_stroke.color,
        );
    }
    resp.clicked()
}

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut extract_clicked = false;
    let mut close = false;
    let mut add_converted = false;
    let mut delete = false;

    card(ui, |ui| {
        let Some(idx) = app.selected else {
            section_label(ui, "INSPECTOR");
            ui.add_space(4.0);
            ui.weak("Select a stream to edit its language, title, flags, and conversion.");
            return;
        };
        let Some(project) = app.project.as_mut() else { return };
        if idx >= project.streams.len() {
            return;
        }

        // Compute the type-relative index and source input before the &mut borrow
        // (both are Copy, so they outlive the borrow of `project.streams`).
        let kind = project.streams[idx].source.kind;
        let input_idx = project.streams[idx].source.input;
        let type_rel = project.streams[..idx]
            .iter()
            .filter(|s| s.source.kind == kind)
            .count();
        let stream = &mut project.streams[idx];

        // Header: title on the left, a close (✕) on the right that deselects.
        ui.horizontal(|ui| {
            section_label(
                ui,
                &format!(
                    "INSPECTOR · {} {} (SELECTED)",
                    super::kind_text(kind).to_uppercase(),
                    type_rel
                ),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if close_button(ui) {
                    close = true;
                }
            });
        });
        ui.add_space(6.0);

        if stream.removed {
            ui.colored_label(RED, "⚠ This stream will be removed from the output.");
            ui.add_space(6.0);
        }

        // Editing fields disable while the stream is marked for removal.
        let editable = !stream.removed;
        ui.add_enabled_ui(editable, |ui| {
            field(ui, "language", |ui| {
                let mut lang = stream.meta.language.clone().unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut lang).desired_width(84.0))
                    .changed()
                {
                    let lang = lang.trim().to_string();
                    stream.meta.language = (!lang.is_empty()).then_some(lang);
                }
            });
            ui.add_space(4.0);
            field(ui, "title", |ui| {
                let mut title = stream.meta.title.clone().unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut title).desired_width(220.0))
                    .changed()
                {
                    stream.meta.title = (!title.is_empty()).then_some(title);
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut stream.meta.default, "default");
                ui.checkbox(&mut stream.meta.forced, "forced");
            });
        });

        // ── Section 2: conversion ────────────────────────────────────────────
        // Only for the file's own streams. An imported track is embedded as-is —
        // re-encoding what you're bringing in doesn't fit that flow — so the whole
        // section is hidden for streams drawn from an added input.
        if input_idx == 0 {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);
            super::section_label(ui, "CONVERSION");
            ui.add_space(6.0);
            ui.add_enabled_ui(!stream.removed, |ui| {
                if kind != Kind::Audio {
                    ui.weak("Only audio streams can be converted.");
                    return;
                }
                if matches!(stream.encode, Encode::Copy) {
                    // A plain copy: offer the two ways to convert.
                    ui.weak("Re-encode this audio to another codec.");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button("convert in place")
                            .on_hover_text("Replace this stream with a re-encoded version (drops the original codec)")
                            .clicked()
                        {
                            stream.encode = Encode::default_audio();
                        }
                        if ui
                            .button("add as new stream")
                            .on_hover_text("Keep this stream and add a converted copy you can reorder and tag")
                            .clicked()
                        {
                            add_converted = true;
                        }
                    });
                } else {
                    // Already converting: tune the codec inline.
                    ui.horizontal(|ui| {
                        field(ui, "codec", |ui| convert_combo(ui, stream));
                        field(ui, "bitrate", |ui| bitrate_combo(ui, stream));
                        field(ui, "channels", |ui| channels_combo(ui, stream));
                    });
                    // An original stream can drop back to stream-copy; a synthetic
                    // added copy can't (that would just duplicate its source) — it's
                    // removed outright in the actions section instead.
                    if !stream.added {
                        ui.add_space(6.0);
                        if ui
                            .button("revert to copy")
                            .on_hover_text("Stop converting; stream-copy the original")
                            .clicked()
                        {
                            stream.encode = Encode::Copy;
                        }
                    }
                }
            });
        }

        // ── Section 3: actions ───────────────────────────────────────────────
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        if stream.added {
            // A synthetic stream isn't in the source file: delete it outright
            // (no soft-removal preview, no revert, no extract).
            let btn = egui::Button::new(egui::RichText::new("🗑 remove stream").color(egui::Color32::WHITE))
                .fill(RED);
            if ui
                .add_sized(egui::vec2(ui.available_width(), 26.0), btn)
                .on_hover_text("Delete this converted stream")
                .clicked()
            {
                delete = true;
            }
        } else {
            // Original stream: a full-width, contextual soft Remove/Restore toggle
            // (red to drop, green to keep), then the Extract row beneath it.
            let (label, fill, hover) = if stream.removed {
                ("↺ restore stream", GREEN, "Keep this stream in the output")
            } else {
                ("🗑 remove stream", RED, "Drop this stream from the output (reversible)")
            };
            let btn = egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE)).fill(fill);
            if ui
                .add_sized(egui::vec2(ui.available_width(), 26.0), btn)
                .on_hover_text(hover)
                .clicked()
            {
                stream.removed = !stream.removed;
            }

            // The `stream` mutable borrow ends here; reborrow `project` for extract.
            let is_removed = project.streams[idx].removed;
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_extract = !is_removed && !matches!(kind, Kind::Attachment | Kind::Data);
                if ui
                    .add_enabled(can_extract, egui::Button::new("extract to file…"))
                    .on_hover_text("Copy just this stream out to its own file")
                    .clicked()
                {
                    extract_clicked = true;
                }
                match project.extract(idx) {
                    Some(x) => {
                        let name = super::file_name_lossy(&x.output).unwrap_or_default();
                        // Truncate to the space left by the button: a long output
                        // name must not widen the inspector panel past the window
                        // (the panel grows to fit its content, then overflows). A
                        // truncated Label shows the full text on hover by itself.
                        ui.add(
                            egui::Label::new(egui::RichText::new(format!("» {name}")).weak())
                                .truncate(),
                        );
                    }
                    None => {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("attachments/data can't be extracted here")
                                    .weak(),
                            )
                            .truncate(),
                        );
                    }
                }
            });
        }

        // ── Section 4: sync offset (embedded tracks only) ────────────────────
        // Only added inputs (index > 0) can be shifted; the primary is the clock
        // everything else syncs against. `stream`'s borrow is dead by here, so we
        // can reborrow `project.inputs`.
        if input_idx != 0 {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);
            super::section_label(ui, "SYNC OFFSET");
            ui.add_space(6.0);
            let input = &mut project.inputs[input_idx];
            ui.horizontal(|ui| {
                if ui.button("−").on_hover_text("Advance 0.05 s").clicked() {
                    input.offset_secs -= 0.05;
                }
                ui.add(
                    egui::DragValue::new(&mut input.offset_secs)
                        .suffix(" s")
                        .speed(0.01)
                        .max_decimals(3)
                        .range(-3600.0..=3600.0),
                );
                if ui.button("+").on_hover_text("Delay 0.05 s").clicked() {
                    input.offset_secs += 0.05;
                }
                if ui.button("⟲").on_hover_text("Reset to 0").clicked() {
                    input.offset_secs = 0.0;
                }
            });
            ui.add_space(4.0);
            ui.weak("+ delays this track, − advances it.");
        }
    });

    if extract_clicked {
        app.extract_selected();
    }
    if add_converted && let Some(i) = app.selected {
        app.add_converted_stream(i);
    }
    if delete && let Some(i) = app.selected {
        app.delete_stream(i);
    }
    if close {
        app.selected = None;
    }
}

/// A captioned field: small label above its widget, matching the mockup.
fn field(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .small()
                .color(egui::Color32::from_gray(150)),
        );
        add(ui);
    });
}

fn convert_combo(ui: &mut egui::Ui, stream: &mut OutStream) {
    let current = match &stream.encode {
        Encode::Copy => "copy".to_string(),
        Encode::Audio { codec, .. } => codec.clone(),
    };
    egui::ComboBox::from_id_salt("convert_to")
        .selected_text(&current)
        .width(84.0)
        .show_ui(ui, |ui| {
            for opt in CODECS {
                if ui.selectable_label(current == opt, opt).clicked() {
                    set_codec(stream, opt);
                }
            }
        });
}

fn bitrate_combo(ui: &mut egui::Ui, stream: &mut OutStream) {
    // Grab the probed source bitrate before the mutable borrow of `encode` so the
    // "auto" label can preview the rung it would follow to.
    let source_kbps = stream.source.bitrate_kbps;
    // Only meaningful for a lossy audio conversion.
    let Encode::Audio { codec, bitrate, .. } = &mut stream.encode else {
        ui.add_enabled(false, egui::Button::new("—"));
        return;
    };
    let lossy = codec != "flac";
    let text = match bitrate {
        // Show the concrete rung AUTO resolves to, e.g. "auto (192k)"; plain
        // "auto" when the source bitrate is unknown (falls back to default).
        Bitrate::Auto => match Bitrate::Auto.resolve(source_kbps) {
            Some(v) => format!("auto ({v}k)"),
            None => "auto".into(),
        },
        Bitrate::Default => "default".into(),
        Bitrate::Fixed(b) => format!("{b}k"),
    };
    ui.add_enabled_ui(lossy, |ui| {
        egui::ComboBox::from_id_salt("bitrate")
            .selected_text(text)
            .width(96.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(*bitrate == Bitrate::Auto, "auto").clicked() {
                    *bitrate = Bitrate::Auto;
                }
                if ui.selectable_label(*bitrate == Bitrate::Default, "default").clicked() {
                    *bitrate = Bitrate::Default;
                }
                for b in BITRATE_LADDER {
                    if ui.selectable_label(*bitrate == Bitrate::Fixed(b), format!("{b}k")).clicked() {
                        *bitrate = Bitrate::Fixed(b);
                    }
                }
            });
    });
}

fn channels_combo(ui: &mut egui::Ui, stream: &mut OutStream) {
    let Encode::Audio { channels, .. } = &mut stream.encode else {
        ui.add_enabled(false, egui::Button::new("—"));
        return;
    };
    let text = CHANNELS
        .iter()
        .find(|(v, _)| v == channels)
        .map(|(_, label)| *label)
        .unwrap_or("source");
    egui::ComboBox::from_id_salt("channels")
        .selected_text(text)
        .width(96.0)
        .show_ui(ui, |ui| {
            for (value, label) in CHANNELS {
                if ui.selectable_label(*channels == value, label).clicked() {
                    *channels = value;
                }
            }
        });
}

/// Switch a stream's encode to `codec` ("copy" reverts to stream-copy),
/// preserving any bitrate/channels already chosen.
fn set_codec(stream: &mut OutStream, codec: &str) {
    if codec == "copy" {
        stream.encode = Encode::Copy;
        return;
    }
    let (bitrate, channels) = match &stream.encode {
        Encode::Audio { bitrate, channels, .. } => (bitrate.clone(), *channels),
        Encode::Copy => (Bitrate::Fixed(192), None),
    };
    stream.encode = Encode::Audio {
        codec: codec.to_string(),
        bitrate,
        channels,
    };
}
