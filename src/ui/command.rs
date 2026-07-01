//! The command bar plus the run/progress footer.
//!
//! The command is a live, read-only render of `Project::to_args()` — the
//! always-visible debugger the brief asks for. (Making it an editable escape
//! hatch, with argv re-parsing, is M5.) The Run button and progress bar are now
//! wired to `run.rs` via `InterlaceApp`'s run-state.

use super::{InterlaceApp, RunState, card, section_label};

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    card(ui, |ui| {
        section_label(ui, "COMMAND (read-only — editable in M5)");
        ui.add_space(4.0);

        let mut text = match &app.project {
            Some(p) => format!("ffmpeg {}", p.to_args().join(" ")),
            None => String::new(),
        };
        ui.add_enabled(
            false,
            egui::TextEdit::multiline(&mut text)
                .font(egui::TextStyle::Monospace)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
    });

    ui.add_space(8.0);
    footer(ui, app);
}

fn footer(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut run_clicked = false;

    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let running = matches!(app.run_state, RunState::Running { .. });
            let enabled = app.project.is_some() && !running;
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
