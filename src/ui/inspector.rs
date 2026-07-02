//! The inspector: the selected row's fields as an editable form. Edits write
//! straight back into `project.streams[selected]` — the table row and inspector
//! are the same object shown two ways.
//!
//! `language`/`title` map an empty field to `None` (inherit the original tag
//! through the copy). The convert controls switch a stream between `Encode::Copy`
//! and `Encode::Audio` and tune its codec/bitrate/channels.

use super::{InterlaceApp, card, section_label};
use crate::model::{Encode, Kind, OutStream};

const CODECS: [&str; 6] = ["copy", "aac", "ac3", "opus", "flac", "mp3"];
const BITRATES: [u32; 6] = [96, 128, 160, 192, 256, 320];
const CHANNELS: [(Option<u32>, &str); 4] = [
    (None, "source"),
    (Some(1), "1 (mono)"),
    (Some(2), "2 (stereo)"),
    (Some(6), "6 (5.1)"),
];

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut extract_clicked = false;
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

        // Compute the type-relative index before taking the &mut borrow.
        let kind = project.streams[idx].source.kind;
        let type_rel = project.streams[..idx]
            .iter()
            .filter(|s| s.source.kind == kind)
            .count();
        let stream = &mut project.streams[idx];

        section_label(
            ui,
            &format!(
                "INSPECTOR · {} {} (SELECTED)",
                super::kind_text(kind).to_uppercase(),
                type_rel
            ),
        );
        ui.add_space(6.0);

        ui.horizontal(|ui| {
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
            field(ui, "title", |ui| {
                let mut title = stream.meta.title.clone().unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut title).desired_width(200.0))
                    .changed()
                {
                    stream.meta.title = (!title.is_empty()).then_some(title);
                }
            });
            field(ui, "convert to", |ui| convert_combo(ui, stream, kind));
            field(ui, "bitrate", |ui| bitrate_combo(ui, stream));
            field(ui, "channels", |ui| channels_combo(ui, stream));
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut stream.meta.default, "default");
            ui.checkbox(&mut stream.meta.forced, "forced");
        });

        // Extract just this stream to its own file. `stream`'s mutable borrow has
        // ended above, so we can reborrow `project` to preview the target name.
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let can_extract = !matches!(kind, Kind::Attachment | Kind::Data);
            if ui
                .add_enabled(can_extract, egui::Button::new("Extract to file…"))
                .on_hover_text("Copy just this stream out to its own file")
                .clicked()
            {
                extract_clicked = true;
            }
            match project.extract(idx) {
                Some(x) => {
                    let name = x
                        .output
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.weak(format!("→ {name}"));
                }
                None => {
                    ui.weak("attachments/data can't be extracted here");
                }
            }
        });
    });

    if extract_clicked {
        app.extract_selected();
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

fn convert_combo(ui: &mut egui::Ui, stream: &mut OutStream, kind: Kind) {
    let is_audio = kind == Kind::Audio;
    let current = match &stream.encode {
        Encode::Copy => "copy".to_string(),
        Encode::Audio { codec, .. } => codec.clone(),
    };
    ui.add_enabled_ui(is_audio, |ui| {
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
    });
}

fn bitrate_combo(ui: &mut egui::Ui, stream: &mut OutStream) {
    // Only meaningful for a lossy audio conversion.
    let Encode::Audio { codec, bitrate_kbps, .. } = &mut stream.encode else {
        ui.add_enabled(false, egui::Button::new("—"));
        return;
    };
    let lossy = codec != "flac";
    let text = bitrate_kbps.map(|b| format!("{b}k")).unwrap_or_else(|| "auto".into());
    ui.add_enabled_ui(lossy, |ui| {
        egui::ComboBox::from_id_salt("bitrate")
            .selected_text(text)
            .width(84.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(bitrate_kbps.is_none(), "auto").clicked() {
                    *bitrate_kbps = None;
                }
                for b in BITRATES {
                    if ui.selectable_label(*bitrate_kbps == Some(b), format!("{b}k")).clicked() {
                        *bitrate_kbps = Some(b);
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
    let (bitrate_kbps, channels) = match &stream.encode {
        Encode::Audio { bitrate_kbps, channels, .. } => (*bitrate_kbps, *channels),
        Encode::Copy => (Some(192), None),
    };
    stream.encode = Encode::Audio {
        codec: codec.to_string(),
        bitrate_kbps,
        channels,
    };
}
