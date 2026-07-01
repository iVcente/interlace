//! The command bar plus the run/progress footer.
//!
//! In M3 the command is a live, read-only render of `Project::to_args()` — the
//! always-visible debugger the brief asks for. Making it editable (and parsing
//! an edited string back into argv as the escape hatch) is M5; wiring the Run
//! button and progress bar to `run.rs` is M4. Both are drawn here but inert.

use super::{InterlaceApp, card, section_label};

pub(super) fn show(ui: &mut egui::Ui, app: &InterlaceApp) {
    card(ui, |ui| {
        section_label(ui, "COMMAND (EDITABLE)");
        ui.add_space(4.0);

        let mut text = match &app.project {
            Some(p) => format!("ffmpeg {}", p.to_args().join(" ")),
            None => String::new(),
        };

        // Disabled in M3 (read-only preview). Becomes an editable escape hatch
        // with argv re-parsing in M5.
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

fn footer(ui: &mut egui::Ui, app: &InterlaceApp) {
    ui.horizontal(|ui| {
        // Run button on the right; progress bar fills the rest on the left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let has_project = app.project.is_some();
            // Inert in M3: the button is drawn but does nothing until M4 wires it.
            ui.add_enabled(false, egui::Button::new("▶ Run"))
                .on_disabled_hover_text("Running is wired up in milestone 4");

            let bar = egui::ProgressBar::new(0.0)
                .desired_width(ui.available_width())
                .text(if has_project { "idle" } else { "no file" });
            ui.add(bar);
        });
    });
}
