//! The inspector: the selected row's fields shown as a form. In M3 the widgets
//! are **disabled previews** (they display the model but don't mutate it) — the
//! table row and inspector are the same object shown two ways. Wiring the edits
//! back into the model is M4.

use super::{InterlaceApp, card, section_label};
use crate::model::{Encode, OutStream};

pub(super) fn show(ui: &mut egui::Ui, app: &InterlaceApp) {
    card(ui, |ui| {
        let Some((idx, stream)) = selected_stream(app) else {
            section_label(ui, "INSPECTOR");
            ui.add_space(4.0);
            ui.weak("Select a stream to edit its language, title, flags, and conversion.");
            return;
        };

        section_label(
            ui,
            &format!(
                "INSPECTOR · {} {} (SELECTED)",
                super::kind_text(stream.source.kind).to_uppercase(),
                type_relative_index(app, idx)
            ),
        );
        ui.add_space(6.0);

        // Everything here is disabled in M3: a faithful preview, not yet editable.
        ui.add_enabled_ui(false, |ui| {
            egui::Grid::new("inspector_fields")
                .num_columns(4)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    labeled(ui, "language", || {});
                    labeled(ui, "title", || {});
                    labeled(ui, "convert to", || {});
                    labeled(ui, "bitrate", || {});
                    ui.end_row();

                    let mut lang = stream.meta.language.clone().unwrap_or_default();
                    ui.add(egui::TextEdit::singleline(&mut lang).desired_width(90.0));

                    let mut title = stream.meta.title.clone().unwrap_or_default();
                    ui.add(egui::TextEdit::singleline(&mut title).desired_width(160.0));

                    let (mut convert, mut bitrate) = convert_fields(stream);
                    ui.add(egui::TextEdit::singleline(&mut convert).desired_width(90.0));
                    ui.add(egui::TextEdit::singleline(&mut bitrate).desired_width(90.0));
                    ui.end_row();
                });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mut default = stream.meta.default;
                ui.checkbox(&mut default, "default");
                let mut forced = stream.meta.forced;
                ui.checkbox(&mut forced, "forced");
            });
        });
    });
}

fn selected_stream(app: &InterlaceApp) -> Option<(usize, &OutStream)> {
    let project = app.project.as_ref()?;
    let idx = app.selected?;
    project.streams.get(idx).map(|s| (idx, s))
}

/// The stream's output index within its own type (a:0, a:1, s:3 …), matching
/// how the serializer numbers it — computed by counting same-kind streams above.
fn type_relative_index(app: &InterlaceApp, idx: usize) -> usize {
    let Some(project) = app.project.as_ref() else {
        return 0;
    };
    let kind = project.streams[idx].source.kind;
    project.streams[..idx]
        .iter()
        .filter(|s| s.source.kind == kind)
        .count()
}

fn convert_fields(stream: &OutStream) -> (String, String) {
    match &stream.encode {
        Encode::Copy => ("copy".into(), String::new()),
        Encode::Audio { codec, bitrate_kbps, .. } => (
            codec.clone(),
            bitrate_kbps.map(|b| format!("{b}k")).unwrap_or_default(),
        ),
    }
}

/// A small field caption (used for the grid's label row).
fn labeled(ui: &mut egui::Ui, text: &str, _add: impl FnOnce()) {
    ui.label(
        egui::RichText::new(text)
            .small()
            .color(egui::Color32::from_gray(150)),
    );
}
