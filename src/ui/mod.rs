//! The egui/eframe view layer.
//!
//! Milestone 3 is a **read-only** skeleton: load a probed file, render the
//! sources bar, the stream table (with selection), a preview inspector, and the
//! live generated command. Editing, drag-reorder, and wiring the Run button to
//! `run.rs` are milestone 4 — the widgets for those are drawn here but inert.
//!
//! `InterlaceApp` owns the whole session state; each screen region lives in its
//! own submodule (`sources`, `table`, `inspector`, `command`) and is handed
//! `&mut InterlaceApp` to render. Shared drawing helpers live here.

mod command;
mod inspector;
mod sources;
mod table;

use crate::model::{Kind, Project};
use std::path::{Path, PathBuf};

/// The whole application state.
pub struct InterlaceApp {
    /// The current editing session; `None` until a file is loaded.
    pub(crate) project: Option<Project>,
    /// Index into `project.streams` of the selected row, if any.
    pub(crate) selected: Option<usize>,
    /// Last error to surface (probe failure, etc.).
    pub(crate) error: Option<String>,
    pub(crate) ffprobe: String,
    #[allow(dead_code)] // used once the Run button is wired (M4)
    pub(crate) ffmpeg: String,
}

impl InterlaceApp {
    /// Construct empty. `CreationContext` is unused for now (default dark theme
    /// is fine) but kept for when we customize fonts/visuals.
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::empty()
    }

    fn empty() -> Self {
        Self {
            project: None,
            selected: None,
            error: None,
            ffprobe: "ffprobe".into(),
            ffmpeg: "ffmpeg".into(),
        }
    }

    /// Probe `path` and make it the current project, selecting the first stream.
    /// A probe failure is surfaced rather than fatal.
    pub fn load_file(&mut self, path: PathBuf) {
        match Project::from_input(&self.ffprobe, &path) {
            Ok(project) => {
                self.selected = (!project.streams.is_empty()).then_some(0);
                self.project = Some(project);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Render one frame into the given root `Ui`. Split out from the trait method
    /// so it can be driven headlessly in tests via `Context::run_ui`.
    ///
    /// eframe 0.35 hands `App::ui` a bare root `Ui` with no background/margin, so
    /// we wrap our content in a `CentralPanel` for the standard framed look.
    fn render(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            header(ui, self);
            ui.add_space(8.0);

            if let Some(err) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("⚠ {err}"));
                ui.add_space(6.0);
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                sources::show(ui, self);
                ui.add_space(8.0);
                table::show(ui, self);
                ui.add_space(8.0);
                inspector::show(ui, self);
                ui.add_space(8.0);
                command::show(ui, self);
            });
        });
    }
}

impl eframe::App for InterlaceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
}

// --- header ------------------------------------------------------------------

fn header(ui: &mut egui::Ui, app: &InterlaceApp) {
    ui.horizontal(|ui| {
        ui.heading("⚙ Stream editor");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = app
                .project
                .as_ref()
                .map(|p| format!("output: {}", container_label(&p.output)))
                .unwrap_or_else(|| "no file loaded".into());
            ui.weak(label);
        });
    });
}

// --- shared drawing helpers (used by the submodules) -------------------------

/// A rounded "card" panel wrapping a section, matching the mockup's grouping.
pub(super) fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .corner_radius(egui::CornerRadius::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// A small uppercase section heading like "SOURCES" / "INSPECTOR".
pub(super) fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .small()
            .strong()
            .color(egui::Color32::from_gray(150)),
    );
}

/// A filled, rounded type badge (video/audio/subtitle color-coded).
pub(super) fn badge(ui: &mut egui::Ui, kind: Kind) {
    egui::Frame::NONE
        .fill(kind_color(kind))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(kind_text(kind))
                    .small()
                    .strong()
                    .color(egui::Color32::WHITE),
            );
        });
}

pub(super) fn kind_color(kind: Kind) -> egui::Color32 {
    match kind {
        Kind::Video => egui::Color32::from_rgb(37, 99, 235),    // blue
        Kind::Audio => egui::Color32::from_rgb(124, 58, 237),   // purple
        Kind::Subtitle => egui::Color32::from_rgb(217, 119, 6), // amber
        Kind::Attachment | Kind::Data => egui::Color32::from_gray(90),
    }
}

pub(super) fn kind_text(kind: Kind) -> &'static str {
    match kind {
        Kind::Video => "video",
        Kind::Audio => "audio",
        Kind::Subtitle => "subtitle",
        Kind::Attachment => "attach",
        Kind::Data => "data",
    }
}

/// Human-friendly container name for the output path's extension.
pub(super) fn container_label(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("mkv") => "Matroska (.mkv)".into(),
        Some("mp4") | Some("m4v") => "MP4 (.mp4)".into(),
        Some("mov") => "QuickTime (.mov)".into(),
        Some("webm") => "WebM (.webm)".into(),
        Some(other) => format!(".{other}"),
        None => "—".into(),
    }
}

/// Open a native file picker for media files. Returns the chosen path, if any.
pub(super) fn pick_media_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(
            "Media",
            &["mkv", "mp4", "m4v", "mov", "webm", "flac", "aac", "ac3", "srt", "ass", "sup"],
        )
        .pick_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Encode, Meta, OutStream, Source};

    fn demo_project() -> Project {
        let mk = |input, index, kind, codec: &str, meta| OutStream {
            source: Source { input, index, kind, codec: codec.into() },
            meta,
            encode: Encode::Copy,
        };
        Project {
            inputs: vec![PathBuf::from("movie.mkv")],
            streams: vec![
                mk(0, 0, Kind::Video, "h264", Meta::default()),
                mk(0, 1, Kind::Audio, "flac", Meta {
                    language: Some("jpn".into()),
                    title: Some("Japanese".into()),
                    default: true,
                    forced: false,
                }),
                mk(0, 2, Kind::Subtitle, "subrip", Meta {
                    language: Some("eng".into()),
                    ..Default::default()
                }),
            ],
            output: PathBuf::from("movie.remux.mkv"),
            duration_secs: Some(120.0),
        }
    }

    /// Render a full frame headlessly with a populated project: exercises every
    /// section's layout code and asserts it doesn't panic.
    #[test]
    fn renders_without_panicking() {
        let mut app = InterlaceApp::empty();
        app.project = Some(demo_project());
        app.selected = Some(1);

        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    /// The empty state (no file) must also render.
    #[test]
    fn renders_empty_state() {
        let mut app = InterlaceApp::empty();
        app.error = Some("something went wrong".into());
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    #[test]
    fn container_label_maps_extensions() {
        assert_eq!(container_label(Path::new("a.mkv")), "Matroska (.mkv)");
        assert_eq!(container_label(Path::new("a.mp4")), "MP4 (.mp4)");
        assert_eq!(container_label(Path::new("a.xyz")), ".xyz");
    }
}
