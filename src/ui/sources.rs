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
            // Pin the action buttons to the right *first*, so they always keep
            // their space; the source chips then truncate into whatever width is
            // left. (Laying the chips out first let a long file name shove the
            // buttons off the row and push the central column wider than the
            // window, which in turn stranded the inspector's resize handle.)
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

                // The chips fill the space left of the buttons, each truncating
                // to fit rather than widening the row.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if let Some(project) = &app.project {
                        for input in &project.inputs {
                            let name = super::file_name_lossy(&input.path)
                                .unwrap_or_else(|| input.path.display().to_string());
                            chip(ui, &name);
                        }
                    }
                });
            });
        });

        // The container title (whole-file metadata) sits under the source chips,
        // once a file is loaded. Empty clears it back to `None`, so an untouched
        // title round-trips through the copy unchanged.
        if let Some(project) = app.project.as_mut() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("title")
                        .small()
                        .color(egui::Color32::from_gray(150)),
                );
                let mut title = project.title.clone().unwrap_or_default();
                let resp = ui
                    .add(
                        egui::TextEdit::singleline(&mut title)
                            .hint_text("file title")
                            .desired_width(f32::INFINITY),
                    )
                    .on_hover_text("Sets the output container's title metadata");
                if resp.changed() {
                    project.title = (!title.is_empty()).then_some(title);
                }
            });
        }

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
            // No leading glyph on the running label — the ● (U+25CF) isn't in
            // egui's default fonts and renders as tofu; an animated spinner beside
            // the button carries the "in progress" cue instead.
            let label = if running { "running…" } else { "▶ run" };
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                run_clicked = true;
            }
            if running {
                ui.add(egui::Spinner::new());
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

/// A read-only rounded chip showing a source file. The name truncates with an
/// ellipsis to fit the width left after the action buttons — so it never widens
/// the row past the window. A truncated `Label` shows the full text on hover on
/// its own (`show_tooltip_when_elided`), so we add no extra tooltip here — that
/// would double it up, and would also show when nothing is elided.
fn chip(ui: &mut egui::Ui, name: &str) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 4))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.add(egui::Label::new(name).truncate());
        });
}
