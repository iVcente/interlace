//! The compatibility findings and the editable command bar. The findings stay in
//! the central column (`show_issues`), while the command bar (`show_command`)
//! lives in a resizable bottom panel toggled by the ⌨ button in the sources bar.
//! (The Run button and progress bar live in the top `sources` bar, alongside the
//! source file.)
//!
//! The command bar mirrors `Project::to_args()` live *until the user edits it* —
//! then it becomes an escape hatch: `InterlaceApp::command_edit` holds the edited
//! text, a "diverges from model" indicator appears, and Run executes that text
//! verbatim (see `run::run_raw`). "Reset to model" drops back to the live view.
//!
//! Above the bar we surface rule-based container/codec compatibility issues from
//! `validate` — but only while following the model, since an edited command is
//! the user's own business.

use super::{InterlaceApp, card, section_label};
use crate::validate::{self, Issue, Severity};

/// The compatibility findings, which stay in the central column so they remain
/// visible even when the command panel is toggled off.
pub(super) fn show_issues(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    // Compatibility findings apply to the model, so hide them once edited.
    if app.command_edit.is_none() && let Some(project) = &app.project {
        let issues = validate::validate(project);
        if !issues.is_empty() {
            issues_card(ui, &issues);
        }
    }
}

/// The editable command bar, rendered into the resizable bottom panel.
pub(super) fn show_command(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    command_card(ui, app);
}

fn command_card(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    // The model's command line, always computed so we can show or diff against it.
    let model_text = match &app.project {
        Some(p) => format!("ffmpeg {}", p.to_args().join(" ")),
        None => String::new(),
    };
    let edited = app.command_edit.is_some();

    card(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "COMMAND");
            if edited {
                ui.label(
                    egui::RichText::new("✎ edited — diverges from model")
                        .small()
                        .color(egui::Color32::from_rgb(217, 119, 6)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Reset to model").clicked() {
                        app.command_edit = None;
                    }
                });
            }
        });
        ui.add_space(4.0);

        // Show the edited buffer if present, otherwise the live model command.
        let mut text = app.command_edit.clone().unwrap_or_else(|| model_text.clone());
        let response = ui.add(
            egui::TextEdit::multiline(&mut text)
                .font(egui::TextStyle::Monospace)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            // First keystroke takes the escape hatch; further edits update it.
            app.command_edit = Some(text);
        }
    });
}

/// A card listing compatibility findings, errors (red) before warnings (amber).
fn issues_card(ui: &mut egui::Ui, issues: &[Issue]) {
    card(ui, |ui| {
        section_label(ui, "COMPATIBILITY");
        ui.add_space(4.0);
        for issue in issues {
            let (icon, color) = match issue.severity {
                Severity::Error => ("⛔", egui::Color32::from_rgb(220, 80, 80)),
                Severity::Warning => ("⚠", egui::Color32::from_rgb(217, 119, 6)),
            };
            let where_ = issue.stream.map(|i| format!("stream {i}: ")).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(color, format!("{icon} {where_}{}", issue.message));
            });
            if let Some(suggestion) = &issue.suggestion {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(18.0);
                    ui.weak(format!("→ {suggestion}"));
                });
            }
            ui.add_space(2.0);
        }
    });
}
