//! The compatibility findings, the command bar, and the run/progress footer.
//!
//! The command bar mirrors `Project::to_args()` live *until the user edits it* —
//! then it becomes an escape hatch: `InterlaceApp::command_edit` holds the edited
//! text, a "diverges from model" indicator appears, and Run executes that text
//! verbatim (see `run::run_raw`). "Reset to model" drops back to the live view.
//!
//! Above the bar we surface rule-based container/codec compatibility issues from
//! `validate` — but only while following the model, since an edited command is
//! the user's own business.

use super::{InterlaceApp, RunState, card, section_label};
use crate::validate::{self, Issue, Severity};

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    // Compatibility findings apply to the model, so hide them once edited.
    if app.command_edit.is_none() {
        if let Some(project) = &app.project {
            let issues = validate::validate(project);
            if !issues.is_empty() {
                issues_card(ui, &issues);
                ui.add_space(8.0);
            }
        }
    }

    command_card(ui, app);

    ui.add_space(8.0);
    footer(ui, app);
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

fn footer(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut run_clicked = false;

    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let running = matches!(app.run_state, RunState::Running { .. });
            // With an edited command we can run even before a file is loaded.
            let have_command = app.project.is_some() || app.command_edit.is_some();
            let enabled = have_command && !running;
            let label = if running { "● Running…" } else { "▶ Run" };
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                run_clicked = true;
            }

            let (fraction, text, color) = progress_view(&app.run_state);
            if let Some(color) = color {
                // Surface a finished-run message beside the bar.
                ui.colored_label(color, &text);
            }
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width(ui.available_width())
                    .text(text),
            );
        });
    });

    if run_clicked {
        app.on_run_clicked();
    }
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

/// Map the run state to (bar fraction, bar text, optional side-message color).
fn progress_view(state: &RunState) -> (f32, String, Option<egui::Color32>) {
    match state {
        RunState::Idle => (0.0, "idle".into(), None),
        RunState::Running { fraction, line, .. } => (*fraction, line.clone(), None),
        RunState::Done { ok: true, line } => {
            (1.0, line.clone(), Some(egui::Color32::from_rgb(80, 200, 120)))
        }
        RunState::Done { ok: false, line } => {
            (0.0, line.clone(), Some(egui::Color32::from_rgb(220, 80, 80)))
        }
    }
}
