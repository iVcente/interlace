//! The top bar: one chip per loaded input file, the "add file" / "add track",
//! command-panel (⌨) and settings (⚙) buttons on the right, and — on the row
//! below — the Run button and progress bar so the primary action sits with the
//! source, not at the bottom of the page.
//!
//! "+ add file" opens a picker and loads it as the primary project (replacing any
//! current one). "+ add track" — shown only once a file is loaded — appends an
//! external audio or subtitle file as an additional input, embedding its single
//! track into the current project (see `InterlaceApp::embed_file`).

use super::{InterlaceApp, RunState, card, pick_media_file, pick_track_file};

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut open_requested = false;
    let mut embed_requested = false;
    let mut toggle_settings = false;
    let mut toggle_command = false;

    card(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(project) = &app.project {
                for input in &project.inputs {
                    let name = input
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| input.path.display().to_string());
                    chip(ui, &name);
                }
            }
            // Add-file/-track and settings sit together on the right of the row.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙").on_hover_text("Binaries / settings").clicked() {
                    toggle_settings = true;
                }
                if ui
                    .selectable_label(app.show_command, "⌨")
                    .on_hover_text("Toggle command panel")
                    .clicked()
                {
                    toggle_command = true;
                }
                if ui.button("+ add file").clicked() {
                    open_requested = true;
                }
                // Embedding needs something to embed into.
                if app.project.is_some()
                    && ui
                        .button("+ add track")
                        .on_hover_text("Embed an external audio or subtitle track")
                        .clicked()
                {
                    embed_requested = true;
                }
            });
        });

        // The Run button and progress bar live right under the source, so the
        // primary action stays with the file it acts on.
        ui.add_space(6.0);
        run_row(ui, app);
    });

    if toggle_settings {
        app.show_settings = !app.show_settings;
    }
    if toggle_command {
        app.show_command = !app.show_command;
    }
    // Run the (blocking) native dialogs after the borrow of `app` ends.
    if open_requested && let Some(path) = pick_media_file() {
        app.load_file(path);
    }
    if embed_requested && let Some(path) = pick_track_file() {
        app.embed_file(path);
    }
}

/// The Run button and its progress bar, sitting under the source chips.
fn run_row(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut run_clicked = false;

    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let running = matches!(app.run_state, RunState::Running { .. });
            // With an edited command we can run even before a file is loaded.
            let have_command = app.project.is_some() || app.command_edit.is_some();
            let enabled = have_command && !running;
            let label = if running { "● running…" } else { "▶ run" };
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

/// A read-only rounded chip showing a source file.
fn chip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 4))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.label(text);
        });
}
