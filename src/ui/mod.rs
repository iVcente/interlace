//! The egui/eframe view layer.
//!
//! Milestone 4 makes it interactive: the inspector edits write back into the
//! model, rows drag-to-reorder, remove/convert actions mutate the stream vec,
//! and the Run button drives `run.rs` with a live progress bar. Extract and an
//! editable command bar are milestone 5.
//!
//! `InterlaceApp` owns the whole session state; each screen region lives in its
//! own submodule (`sources`, `table`, `inspector`, `command`) and is handed
//! `&mut InterlaceApp` to render. Shared drawing helpers live here.

mod command;
mod inspector;
mod sources;
mod table;

use crate::model::{Encode, Kind, Project};
use crate::run::{self, RunUpdate};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

/// The state of the current (or last) ffmpeg run.
pub(crate) enum RunState {
    Idle,
    Running {
        rx: Receiver<RunUpdate>,
        fraction: f32,
        line: String,
    },
    Done {
        ok: bool,
        line: String,
    },
}

/// The whole application state.
pub struct InterlaceApp {
    /// The current editing session; `None` until a file is loaded.
    pub(crate) project: Option<Project>,
    /// Index into `project.streams` of the selected row, if any.
    pub(crate) selected: Option<usize>,
    /// Last error to surface (probe failure, etc.).
    pub(crate) error: Option<String>,
    pub(crate) run_state: RunState,
    /// Set when Run is pressed but the output already exists — drives a modal.
    pub(crate) pending_overwrite: bool,
    /// The command-bar escape hatch. `None` means "follow the model" (the bar
    /// mirrors `to_args()` live); `Some` means the user has edited it, so Run
    /// executes this text verbatim instead. Reset to `None` on load or "Reset".
    pub(crate) command_edit: Option<String>,
    pub(crate) ffprobe: String,
    pub(crate) ffmpeg: String,
}

impl InterlaceApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::empty()
    }

    fn empty() -> Self {
        Self {
            project: None,
            selected: None,
            error: None,
            run_state: RunState::Idle,
            pending_overwrite: false,
            command_edit: None,
            ffprobe: "ffprobe".into(),
            ffmpeg: "ffmpeg".into(),
        }
    }

    /// Probe `path` and make it the current project, selecting the first stream.
    pub fn load_file(&mut self, path: PathBuf) {
        match Project::from_input(&self.ffprobe, &path) {
            Ok(project) => {
                self.selected = (!project.streams.is_empty()).then_some(0);
                self.project = Some(project);
                self.error = None;
                self.run_state = RunState::Idle;
                self.command_edit = None; // a new file starts from the model

            }
            Err(e) => self.error = Some(e),
        }
    }

    // --- model mutations (called by the table/inspector after rendering) -----

    pub(crate) fn remove_stream(&mut self, i: usize) {
        let Some(p) = &mut self.project else { return };
        if i >= p.streams.len() {
            return;
        }
        p.streams.remove(i);
        let len = p.streams.len();
        self.selected = match self.selected {
            _ if len == 0 => None,
            Some(s) if s > i => Some(s - 1),
            Some(s) if s == i => Some(i.min(len - 1)),
            other => other, // s < i, or None
        };
    }

    /// Toggle an audio stream between copy and a default AAC conversion.
    pub(crate) fn toggle_convert(&mut self, i: usize) {
        let Some(p) = &mut self.project else { return };
        let Some(s) = p.streams.get_mut(i) else { return };
        if s.source.kind != Kind::Audio {
            return;
        }
        s.encode = match s.encode {
            Encode::Copy => Encode::Audio {
                codec: "aac".into(),
                bitrate_kbps: Some(192),
                channels: None,
            },
            Encode::Audio { .. } => Encode::Copy,
        };
        self.selected = Some(i);
    }

    /// Move the stream at `from` to insertion index `to` (0..=len), keeping the
    /// moved stream selected. `to` is expressed in the pre-removal coordinate
    /// space, as computed from the drop pointer position.
    pub(crate) fn reorder(&mut self, from: usize, to: usize) {
        let Some(p) = &mut self.project else { return };
        if from >= p.streams.len() || to > p.streams.len() {
            return;
        }
        let item = p.streams.remove(from);
        let insert = if from < to { to - 1 } else { to };
        let insert = insert.min(p.streams.len());
        p.streams.insert(insert, item);
        self.selected = Some(insert);
    }

    // --- running -------------------------------------------------------------

    /// Called when Run is pressed: confirm overwrite if needed, else start.
    pub(crate) fn on_run_clicked(&mut self) {
        if matches!(self.run_state, RunState::Running { .. }) {
            return;
        }
        // Escape hatch: an edited command runs verbatim, bypassing the model
        // (and thus the model-based overwrite pre-check — the user owns `-y`).
        if let Some(command) = self.command_edit.clone() {
            self.start_run_edited(&command);
            return;
        }
        let Some(p) = &self.project else { return };
        if p.output.exists() {
            self.pending_overwrite = true;
        } else {
            self.start_run(false);
        }
    }

    /// Run a user-edited command string. Tokenizes it, drops a leading `ffmpeg`
    /// program token (the configured binary is authoritative), and runs the rest.
    pub(crate) fn start_run_edited(&mut self, command: &str) {
        let args = command_args(command);
        let duration = self.project.as_ref().and_then(|p| p.duration_secs);
        self.run_state = match run::run_raw(&self.ffmpeg, args, duration) {
            Ok(rx) => RunState::Running { rx, fraction: 0.0, line: "starting…".into() },
            Err(e) => RunState::Done { ok: false, line: e },
        };
    }

    fn start_run(&mut self, overwrite: bool) {
        let Some(project) = self.project.clone() else { return };
        self.run_state = match run::run(&self.ffmpeg, &project, overwrite) {
            Ok(rx) => RunState::Running {
                rx,
                fraction: 0.0,
                line: "starting…".into(),
            },
            Err(e) => RunState::Done { ok: false, line: e },
        };
    }

    /// Drain progress updates each frame while a run is active.
    fn poll_run(&mut self, ctx: &egui::Context) {
        let RunState::Running { rx, fraction, line } = &mut self.run_state else {
            return;
        };
        let mut finished: Option<(bool, String)> = None;
        loop {
            match rx.try_recv() {
                Ok(RunUpdate::Progress(p)) => {
                    if let Some(f) = p.fraction {
                        *fraction = f as f32;
                    }
                    *line = progress_line(&p);
                }
                Ok(RunUpdate::Finished { result }) => {
                    finished = Some(match result {
                        Ok(()) => (true, "done".into()),
                        Err(f) => (false, failure_line(&f)),
                    });
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some((false, "run ended unexpectedly".into()));
                    break;
                }
            }
        }
        // Keep animating while a run is live (egui otherwise sleeps until input).
        ctx.request_repaint();
        if let Some((ok, line)) = finished {
            self.run_state = RunState::Done { ok, line };
        }
    }

    // --- frame ---------------------------------------------------------------

    /// Render one frame into the given root `Ui`. Split out from the trait method
    /// so it can be driven headlessly in tests via `Context::run_ui`.
    fn render(&mut self, ui: &mut egui::Ui) {
        self.poll_run(ui.ctx());

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

        self.overwrite_modal(ui.ctx());
    }

    fn overwrite_modal(&mut self, ctx: &egui::Context) {
        if !self.pending_overwrite {
            return;
        }
        let path = self
            .project
            .as_ref()
            .map(|p| p.output.display().to_string())
            .unwrap_or_default();

        let (mut start, mut cancel) = (false, false);
        egui::Window::new("⚠ Output file exists")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("{path}\n\nOverwrite it?"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Overwrite").clicked() {
                        start = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if start {
            self.pending_overwrite = false;
            self.start_run(true);
        } else if cancel {
            self.pending_overwrite = false;
        }
    }
}

impl eframe::App for InterlaceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
}

/// Turn an edited command line into ffmpeg argv: tokenize, then drop a leading
/// `ffmpeg` / `ffmpeg.exe` program token so the configured binary stays in
/// charge (the user edits the *arguments*, not which executable we launch).
fn command_args(command: &str) -> Vec<String> {
    let mut args = run::tokenize(command);
    let leads_with_ffmpeg = args
        .first()
        .map(|a| a.eq_ignore_ascii_case("ffmpeg") || a.to_lowercase().ends_with("ffmpeg.exe"))
        .unwrap_or(false);
    if leads_with_ffmpeg {
        args.remove(0);
    }
    args
}

fn progress_line(p: &run::Progress) -> String {
    let pct = p
        .fraction
        .map(|f| format!("{:.0}%", f * 100.0))
        .unwrap_or_else(|| "—".into());
    let speed = p.speed.map(|s| format!("{s:.1}x")).unwrap_or_default();
    format!("{pct}   ·   t={:.0}s   ·   {speed}", p.out_time_secs)
}

fn failure_line(f: &run::RunFailure) -> String {
    // The last stderr line is almost always the actual error.
    let last = f.stderr_tail.lines().last().unwrap_or("ffmpeg failed");
    let code = f.code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
    format!("failed (exit {code}): {last}")
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
    use crate::model::{Meta, OutStream, Source};

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

    fn app_with_demo() -> InterlaceApp {
        let mut app = InterlaceApp::empty();
        app.project = Some(demo_project());
        app.selected = Some(1);
        app
    }

    #[test]
    fn renders_without_panicking() {
        let mut app = app_with_demo();
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    #[test]
    fn renders_empty_state() {
        let mut app = InterlaceApp::empty();
        app.error = Some("something went wrong".into());
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    #[test]
    fn reorder_moves_and_follows_selection() {
        let mut app = app_with_demo();
        // Move the subtitle (idx 2) to the top (insertion index 0).
        app.reorder(2, 0);
        let streams = &app.project.as_ref().unwrap().streams;
        assert_eq!(streams[0].source.kind, Kind::Subtitle);
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn remove_fixes_selection() {
        let mut app = app_with_demo(); // selected = 1
        app.remove_stream(0); // removing above selection shifts it down
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.project.as_ref().unwrap().streams.len(), 2);
    }

    #[test]
    fn convert_toggles_audio_only() {
        let mut app = app_with_demo();
        app.toggle_convert(1); // audio → convert
        assert!(matches!(
            app.project.as_ref().unwrap().streams[1].encode,
            Encode::Audio { .. }
        ));
        app.toggle_convert(0); // video → no-op
        assert!(matches!(
            app.project.as_ref().unwrap().streams[0].encode,
            Encode::Copy
        ));
    }

    #[test]
    fn command_args_strips_leading_ffmpeg_program_token() {
        assert_eq!(
            command_args(r#"ffmpeg -i "a b.mkv" -c copy out.mkv"#),
            vec!["-i", "a b.mkv", "-c", "copy", "out.mkv"]
        );
        // Case/extension variants of the program token are stripped too.
        assert_eq!(command_args("FFMPEG.exe -version"), vec!["-version"]);
        // A command that doesn't start with ffmpeg is passed through as-is.
        assert_eq!(command_args("-i in.mkv"), vec!["-i", "in.mkv"]);
    }

    #[test]
    fn renders_edited_command_bar_without_panicking() {
        let mut app = app_with_demo();
        app.command_edit = Some("ffmpeg -i in.mkv -c copy out.mkv".into());
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
