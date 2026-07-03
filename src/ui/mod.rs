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

use crate::model::{Encode, Kind, OutStream, Project};
use crate::run::{self, RunUpdate};
use std::path::PathBuf;
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
    /// A run awaiting overwrite confirmation. Holds the exact project to run
    /// (the main remux, or a one-off extract) so the modal can start it as-is.
    pub(crate) pending_run: Option<Project>,
    /// The command-bar escape hatch. `None` means "follow the model" (the bar
    /// mirrors `to_args()` live); `Some` means the user has edited it, so Run
    /// executes this text verbatim instead. Reset to `None` on load or "Reset".
    pub(crate) command_edit: Option<String>,
    pub(crate) ffprobe: String,
    pub(crate) ffmpeg: String,
    /// Cached results of the last `<bin> -version` probe (`Ok(version-line)` or a
    /// legible error). `Ok("")` means "not yet checked" (the test default).
    pub(crate) ffmpeg_status: Result<String, String>,
    pub(crate) ffprobe_status: Result<String, String>,
    /// Whether the binaries settings panel is expanded.
    pub(crate) show_settings: bool,
}

impl InterlaceApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::empty();
        app.recheck_binaries(); // detect ffmpeg/ffprobe up front
        app
    }

    fn empty() -> Self {
        Self {
            project: None,
            selected: None,
            error: None,
            run_state: RunState::Idle,
            pending_run: None,
            command_edit: None,
            ffprobe: "ffprobe".into(),
            ffmpeg: "ffmpeg".into(),
            ffmpeg_status: Ok(String::new()),
            ffprobe_status: Ok(String::new()),
            show_settings: false,
        }
    }

    /// Re-probe both binaries and cache their status (called at startup and from
    /// the settings "Re-check" button after an override path is edited).
    pub(crate) fn recheck_binaries(&mut self) {
        self.ffmpeg_status = run::version(&self.ffmpeg);
        self.ffprobe_status = run::version(&self.ffprobe);
    }

    /// Both binaries resolved on the last check.
    pub(crate) fn binaries_ok(&self) -> bool {
        self.ffmpeg_status.is_ok() && self.ffprobe_status.is_ok()
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

    /// Add a re-encoded sibling of `streams[i]`: the same source, a default AAC
    /// conversion, inserted right after the original (which stays a stream-copy)
    /// and selected — so its codec, tags, and order are tuned like any stream.
    pub(crate) fn add_converted_stream(&mut self, i: usize) {
        let Some(p) = &mut self.project else { return };
        let Some(orig) = p.streams.get(i) else { return };
        // Seed the copy's tags from the original, but clear default/forced so we
        // don't emit two "default" streams for the same track.
        let mut meta = orig.meta.clone();
        meta.default = false;
        meta.forced = false;
        let mut converted = OutStream::new(orig.source.clone(), meta, Encode::default_audio());
        converted.added = true; // synthetic — deleted outright, not soft-removed
        let at = (i + 1).min(p.streams.len());
        p.streams.insert(at, converted);
        self.selected = Some(at);
    }

    /// Hard-remove `streams[i]` from the project, fixing the selection to a
    /// surviving stream. Used for synthetic (`added`) streams, which aren't in
    /// the source file so there's nothing to preview as a "soft" removal.
    pub(crate) fn delete_stream(&mut self, i: usize) {
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
        // Escape hatch: an edited command runs verbatim, bypassing the model
        // (and thus the model-based overwrite pre-check — the user owns `-y`).
        if let Some(command) = self.command_edit.clone() {
            if !matches!(self.run_state, RunState::Running { .. }) {
                self.start_run_edited(&command);
            }
            return;
        }
        if let Some(p) = self.project.clone() {
            self.begin_run(p);
        }
    }

    /// Extract the selected stream to its own file, running it as a one-off job.
    pub(crate) fn extract_selected(&mut self) {
        let Some(idx) = self.selected else { return };
        let Some(project) = &self.project else { return };
        match project.extract(idx) {
            Some(extract) => {
                self.error = None;
                self.begin_run(extract);
            }
            None => self.error = Some("This stream type can't be extracted.".into()),
        }
    }

    /// Start `project`, confirming first if its output already exists.
    fn begin_run(&mut self, project: Project) {
        if matches!(self.run_state, RunState::Running { .. }) {
            return;
        }
        if project.output.exists() {
            self.pending_run = Some(project);
        } else {
            self.start_run_project(project, false);
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

    fn start_run_project(&mut self, project: Project, overwrite: bool) {
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

        // The inspector is a right-hand panel. It's added before the central panel
        // so egui reserves its width first; the table/command flow fills the rest.
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(320.0)
            .size_range(260.0..=460.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    inspector::show(ui, self);
                });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            sources::show(ui, self);
            ui.add_space(8.0);

            let red = egui::Color32::from_rgb(220, 80, 80);
            if !self.binaries_ok() {
                ui.colored_label(red, "⚠ ffmpeg/ffprobe not found — open ⚙ to set the path.");
                ui.add_space(6.0);
            }
            if let Some(err) = &self.error {
                ui.colored_label(red, format!("⚠ {err}"));
                ui.add_space(6.0);
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.show_settings {
                    settings_card(ui, self);
                    ui.add_space(8.0);
                }
                table::show(ui, self);
                ui.add_space(8.0);
                command::show(ui, self);
            });
        });

        self.overwrite_modal(ui.ctx());
    }

    fn overwrite_modal(&mut self, ctx: &egui::Context) {
        let Some(project) = &self.pending_run else { return };
        let path = project.output.display().to_string();

        let (mut start, mut cancel) = (false, false);
        egui::Window::new("⚠ output file exists")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("{path}\n\noverwrite it?"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("overwrite").clicked() {
                        start = true;
                    }
                    if ui.button("cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if start {
            if let Some(project) = self.pending_run.take() {
                self.start_run_project(project, true);
            }
        } else if cancel {
            self.pending_run = None;
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

/// The collapsible binaries panel: editable ffmpeg/ffprobe paths, their detected
/// version (or error), and a button to re-probe after editing.
fn settings_card(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    card(ui, |ui| {
        section_label(ui, "BINARIES");
        ui.add_space(4.0);
        ui.weak("A bare name resolves via PATH; enter a full path to override.");
        ui.add_space(6.0);

        egui::Grid::new("binaries_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("ffmpeg");
                ui.text_edit_singleline(&mut app.ffmpeg);
                ui.end_row();
                ui.label("");
                status_line(ui, &app.ffmpeg_status);
                ui.end_row();

                ui.label("ffprobe");
                ui.text_edit_singleline(&mut app.ffprobe);
                ui.end_row();
                ui.label("");
                status_line(ui, &app.ffprobe_status);
                ui.end_row();
            });

        ui.add_space(6.0);
        if ui.button("re-check").clicked() {
            app.recheck_binaries();
        }
    });
}

/// Render a binary's last probe result: green version, red error, or "not checked".
fn status_line(ui: &mut egui::Ui, status: &Result<String, String>) {
    match status {
        Ok(v) if v.is_empty() => {
            ui.weak("not checked");
        }
        Ok(v) => {
            ui.colored_label(egui::Color32::from_rgb(80, 200, 120), format!("✓ {v}"));
        }
        Err(e) => {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("✗ {e}"));
        }
    }
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
    use crate::model::{Meta, Source};

    fn demo_project() -> Project {
        let mk = |input, index, kind, codec: &str, meta| {
            OutStream::new(
                Source { input, index, kind, codec: codec.into() },
                meta,
                Encode::Copy,
            )
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
    fn add_converted_stream_keeps_original_and_inserts_sibling() {
        let mut app = app_with_demo(); // selected = 1 (the flac audio, default+jpn)
        app.add_converted_stream(1);
        let p = app.project.as_ref().unwrap();

        assert_eq!(p.streams.len(), 4);
        // Original stays a stream-copy in place.
        assert!(matches!(p.streams[1].encode, Encode::Copy));
        assert_eq!(p.streams[1].source.index, 1);
        // Converted sibling inserted right after: same source, audio re-encode,
        // tags seeded but default cleared. It becomes the selection.
        assert!(matches!(p.streams[2].encode, Encode::Audio { .. }));
        assert_eq!(p.streams[2].source.index, 1);
        assert_eq!(p.streams[2].meta.language.as_deref(), Some("jpn"));
        assert!(!p.streams[2].meta.default);
        assert_eq!(app.selected, Some(2));

        // Both map from the same input stream → the source is emitted twice.
        let maps: Vec<String> = p
            .to_args()
            .windows(2)
            .filter(|w| w[0] == "-map")
            .map(|w| w[1].clone())
            .collect();
        assert_eq!(maps.iter().filter(|m| *m == "0:1").count(), 2);
    }

    #[test]
    fn delete_stream_hard_removes_added_sibling() {
        let mut app = app_with_demo(); // 3 streams, selected = 1
        app.add_converted_stream(1); // now 4 streams, added sibling selected at 2
        assert!(app.project.as_ref().unwrap().streams[2].added);
        app.delete_stream(2); // hard delete the synthetic stream
        let p = app.project.as_ref().unwrap();
        assert_eq!(p.streams.len(), 3); // back to the original three
        assert!(!p.streams.iter().any(|s| s.added));
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
    fn renders_removed_and_retagged_stream() {
        // A removed row (dimmed + "remove" badge) and a retagged row exercise the
        // table's change-summary path and the inspector's removed state together.
        let mut app = app_with_demo();
        {
            let streams = &mut app.project.as_mut().unwrap().streams;
            streams[1].removed = true; // pending removal → red badge, struck row
            streams[2].meta.title = Some("Forced".into()); // retag vs orig_meta
        }
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    #[test]
    fn renders_settings_panel_and_missing_binary_banner() {
        let mut app = app_with_demo();
        app.show_settings = true;
        app.ffmpeg_status = Err("not found (`ffmpeg`)".into());
        app.ffprobe_status = Ok("ffprobe version 8.1".into());
        assert!(!app.binaries_ok());
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    #[test]
    fn extract_of_attachment_sets_error() {
        let mut app = app_with_demo();
        // Append an attachment and select it; extracting it isn't supported.
        let attach = OutStream::new(
            Source { input: 0, index: 9, kind: Kind::Attachment, codec: "ttf".into() },
            Meta::default(),
            Encode::Copy,
        );
        app.project.as_mut().unwrap().streams.push(attach);
        app.selected = Some(app.project.as_ref().unwrap().streams.len() - 1);
        app.extract_selected();
        assert!(app.error.is_some());
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
}
