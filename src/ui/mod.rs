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
//!
//! The table selection is a set (`selection::Selection`), so the inspector and
//! the remove/restore/convert mutations below work over every selected row at
//! once. Single-stream-only actions (extract, sync offset) need exactly one.
//!
//! Media enters through the sources-bar pickers or by dropping files onto the
//! window. Drops reuse the picker paths (`load_file` / `embed_file`), routed by
//! extension against the same `TRACK_EXTENSIONS` list the track picker filters on.
//!
//! The subtitle cue viewer (`cues`) docks under the inspector in the right-hand
//! panel, opened from the inspector for one text subtitle stream.

mod command;
mod cues;
mod inspector;
mod selection;
mod sources;
mod table;

use crate::model::{Encode, Kind, OutStream, Project};
use selection::Selection;
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
        /// A preview clip to open in the default player once the run succeeds.
        open_on_success: Option<PathBuf>,
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
    /// The selected rows of the stream table (indices into `project.streams`).
    pub(crate) selection: Selection,
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
    /// The subtitle cue viewer (`cues.rs`); `None` until one is opened. Kept when
    /// its window closes so reopening the same stream needn't re-read the file.
    pub(crate) cues: Option<cues::CueView>,
    /// The preview clip's start time as typed (`hh:mm:ss`, `mm:ss` or seconds),
    /// in output time. Kept as text so a half-typed value isn't reformatted under
    /// the cursor; clicking a cue in the viewer writes it.
    pub(crate) preview_start: String,
    /// The preview clip's length in seconds, one of `preview::LENGTHS`.
    pub(crate) preview_len: f64,
    /// Every preview clip rendered this session, deleted when the app closes.
    preview_clips: Vec<PathBuf>,
    pub(crate) ffprobe: String,
    pub(crate) ffmpeg: String,
    /// Cached results of the last `<bin> -version` probe (`Ok(version-line)` or a
    /// legible error). `Ok("")` means "not yet checked" (the test default).
    pub(crate) ffmpeg_status: Result<String, String>,
    pub(crate) ffprobe_status: Result<String, String>,
    /// Whether the binaries settings panel is expanded.
    pub(crate) show_settings: bool,
    /// Whether the command bar (a resizable bottom panel) is shown.
    pub(crate) show_command: bool,
    /// `show_command` as of the last rendered frame. Adding/removing a panel
    /// doesn't trigger egui's settle-pass the way changing central content does,
    /// so on the frame this differs we force a discard to avoid a one-frame
    /// flash of id-clash outlines while the layout re-settles.
    prev_show_command: bool,
}

impl InterlaceApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::empty();
        app.recheck_binaries(); // detect ffmpeg/ffprobe up front
        crate::preview::sweep_temp(); // clips left over from earlier sessions
        app
    }

    fn empty() -> Self {
        Self {
            project: None,
            selection: Selection::default(),
            error: None,
            run_state: RunState::Idle,
            pending_run: None,
            command_edit: None,
            cues: None,
            preview_start: "00:00:00".into(),
            preview_len: 30.0,
            preview_clips: Vec::new(),
            ffprobe: "ffprobe".into(),
            ffmpeg: "ffmpeg".into(),
            ffmpeg_status: Ok(String::new()),
            ffprobe_status: Ok(String::new()),
            show_settings: false,
            show_command: false,
            prev_show_command: false,
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
                self.selection.clear();
                if !project.streams.is_empty() {
                    self.selection.set_single(0);
                }
                self.project = Some(project);
                self.error = None;
                self.run_state = RunState::Idle;
                self.command_edit = None; // a new file starts from the model
                self.cues = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Embed an external audio/subtitle track from `path` as a new input of the
    /// current project, selecting it. With no project loaded there's nothing to
    /// embed into, so we treat it as opening a primary file instead. A hand-edited
    /// command is left untouched (the model diverging from it is expected).
    pub fn embed_file(&mut self, path: PathBuf) {
        let Some(project) = self.project.as_mut() else {
            self.load_file(path);
            return;
        };
        match project.append_input(&self.ffprobe, &path) {
            Ok(i) => {
                self.selection.set_single(i);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Handle files dropped onto the window, routing each through the same path
    /// as the pickers: a container opens via `load_file`, a track embeds via
    /// `embed_file` (which itself opens it when no project is loaded yet). Each
    /// call's error is collected as we go — otherwise a later success clearing
    /// `error` would hide an earlier failure.
    pub(crate) fn open_dropped(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if matches!(self.run_state, RunState::Running { .. }) {
            self.error = Some("Can't change sources while a run is in progress.".into());
            return;
        }
        let (ordered, skipped) = order_drops(paths);
        let mut errors = Vec::new();
        for path in ordered {
            if is_track_file(&path) {
                self.embed_file(path);
            } else {
                self.load_file(path);
            }
            errors.extend(self.error.take());
        }
        for path in skipped {
            errors.push(format!("{} skipped — only one file can be opened at a time.", path.display()));
        }
        self.error = (!errors.is_empty()).then(|| errors.join("\n"));
    }

    // --- model mutations (called by the table/inspector after rendering) -----

    /// Add a re-encoded sibling of each stream in `rows`: the same source, a
    /// default AAC conversion, inserted right after its original (which stays a
    /// stream-copy). The new siblings become the selection, so their codec, tags,
    /// and order are tuned next like any stream. `rows` must be ascending.
    pub(crate) fn add_converted_streams(&mut self, rows: &[usize]) {
        let Some(p) = &mut self.project else { return };
        let rows: Vec<usize> = rows.iter().copied().filter(|&i| i < p.streams.len()).collect();
        // Insert from the highest index down so each insert leaves the lower,
        // not-yet-visited originals where they are.
        for &i in rows.iter().rev() {
            let orig = &p.streams[i];
            // Seed the copy's tags from the original, but clear default/forced so
            // we don't emit two "default" streams for the same track.
            let mut meta = orig.meta.clone();
            meta.default = false;
            meta.forced = false;
            let mut converted = OutStream::new(orig.source.clone(), meta, Encode::default_audio());
            converted.added = true; // synthetic — deleted outright, not soft-removed
            p.streams.insert(i + 1, converted);
        }
        // The k-th original (ascending) ends up shifted by the k siblings inserted
        // before it, so its sibling sits at `i + 1 + k`.
        self.selection
            .set_many(rows.iter().enumerate().map(|(k, &i)| i + 1 + k));
    }

    /// Remove every selected stream: originals are soft-removed (dimmed, kept for
    /// Restore), synthetic `added` ones are deleted outright — the same thing the
    /// inspector's remove button does for each on its own. Driven by that button
    /// and the Delete key.
    pub(crate) fn remove_selected(&mut self) {
        let Some(p) = &mut self.project else { return };
        let mut deleted = false;
        // Walk high→low so a hard delete only shifts rows we've already handled.
        for i in self.selection.to_vec().into_iter().rev() {
            let Some(stream) = p.streams.get_mut(i) else { continue };
            if stream.added {
                p.streams.remove(i);
                self.selection.on_removed(i, p.streams.len());
                deleted = true;
            } else {
                stream.removed = true;
            }
        }
        if deleted {
            // Dropping the last stream of an embedded track also drops its `-i`.
            p.prune_orphan_inputs();
        }
    }

    /// Undo a soft removal on every selected stream.
    pub(crate) fn restore_selected(&mut self) {
        let Some(p) = &mut self.project else { return };
        for i in self.selection.iter() {
            if let Some(stream) = p.streams.get_mut(i) {
                stream.removed = false;
            }
        }
    }

    /// Move the stream at `from` to insertion index `to` (0..=len). The selection
    /// collapses to the moved stream: a drag moves one row, so that row is what
    /// the user is focused on. `to` is expressed in the pre-removal coordinate
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
        self.selection.set_single(insert);
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
    /// Needs exactly one selected stream; a batch extract would need a run queue.
    pub(crate) fn extract_selected(&mut self) {
        let Some(idx) = self.selection.single() else { return };
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
        self.run_state = run_state_from(run::run_raw(&self.ffmpeg, args, duration), None);
    }

    fn start_run_project(&mut self, project: Project, overwrite: bool) {
        self.run_state = run_state_from(run::run(&self.ffmpeg, &project, overwrite), None);
    }

    /// Why a preview can't start right now, or `None` when it can. Drives both
    /// the button's enabled state and its hover text.
    pub(crate) fn preview_blocker(&self) -> Option<&'static str> {
        if self.project.is_none() {
            Some("Open a file first")
        } else if matches!(self.run_state, RunState::Running { .. }) {
            Some("Wait for the current run to finish")
        } else if self.command_edit.is_some() {
            Some("The preview follows the editor, not the edited command — reset the command first")
        } else if crate::preview::parse_timestamp(&self.preview_start).is_none() {
            Some("Enter a start time like 00:12:30, 12:30 or 750")
        } else {
            None
        }
    }

    /// Render a `preview_len`-second clip of the current project from
    /// `preview_start` and open it in the default player when it's done. The
    /// single selected audio/subtitle stream is made the default of its kind so
    /// the player starts on it (see `preview::focused`).
    pub(crate) fn start_preview(&mut self) {
        if self.preview_blocker().is_some() {
            return;
        }
        let (Some(project), Some(start)) = (&self.project, crate::preview::parse_timestamp(&self.preview_start))
        else {
            return;
        };
        let clip = crate::preview::focused(project, self.selection.single());
        let out = crate::preview::next_path();
        let args = clip.preview_args(start, self.preview_len, &out);
        self.preview_clips.push(out.clone());
        self.error = None;
        self.run_state = run_state_from(run::run_preview(&self.ffmpeg, args, self.preview_len), Some(out));
    }

    /// Drain progress updates each frame while a run is active.
    fn poll_run(&mut self, ctx: &egui::Context) {
        let RunState::Running {
            rx,
            fraction,
            line,
            open_on_success,
        } = &mut self.run_state
        else {
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
                    finished = Some(match (result, open_on_success.as_deref()) {
                        (Ok(()), None) => (true, "done".into()),
                        (Ok(()), Some(clip)) => match run::open_in_default_app(clip) {
                            Ok(()) => (true, "preview opened".into()),
                            Err(e) => (false, e),
                        },
                        (Err(f), _) => (false, failure_line(&f)),
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
        cues::poll(ui.ctx(), self);

        // Files dropped onto the window this frame. Entries without a path (web
        // drops carrying only bytes) and folders have nothing for ffprobe to read.
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .filter(|p| !p.is_dir())
                .collect()
        });
        self.open_dropped(dropped);

        // Toggling a panel on/off doesn't trigger egui's automatic settle-pass
        // (unlike changing central content), so the first frame after a toggle is
        // laid out unsettled and paints a flash of red id-clash outlines. Force a
        // discard on that frame so egui re-runs the pass cleanly, as it would for
        // any other layout change. `prev_show_command` updates now (state survives
        // the discard) so the re-run pass sees no change and paints normally.
        if self.show_command != self.prev_show_command {
            self.prev_show_command = self.show_command;
            ui.ctx().request_discard("command panel toggled");
        }

        // The inspector is a right-hand panel. It's added before the central panel
        // so egui reserves its width first; the table/command flow fills the rest.
        egui::Panel::right("inspector")
            .resizable(true)
            .show_separator_line(false) // keep the drag, drop the visible line
            .default_size(320.0)
            .size_range(260.0..=460.0)
            .show(ui, |ui| {
                // With the cue panel docked below, the inspector scrolls within
                // the top part and the cues take the rest; on its own it can use
                // the full height. Either way it shrinks to its content.
                let docked = cues::is_open(self);
                let max = if docked { ui.available_height() * 0.6 } else { f32::INFINITY };
                egui::ScrollArea::vertical()
                    .id_salt("inspector_scroll")
                    .max_height(max)
                    .show(ui, |ui| {
                        inspector::show(ui, self);
                    });
                if docked {
                    ui.add_space(8.0);
                    cues::panel(ui, self);
                }
            });

        // The command bar is a resizable bottom panel, like the inspector on the
        // right. Added after the inspector so it spans only the central column's
        // width; toggled by the ⌨ button in the sources bar.
        if self.show_command {
            egui::Panel::bottom("command")
                .resizable(true)
                .show_separator_line(false) // keep the drag, drop the visible line
                .default_size(150.0)
                .size_range(90.0..=420.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        command::show_command(ui, self);
                    });
                });
        }

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
                command::show_issues(ui, self);
            });
        });

        self.overwrite_modal(ui.ctx());
        self.drop_overlay(ui.ctx());
    }

    /// While files are dragged over the window, dim it and say what a drop will
    /// do — otherwise there's no cue that the window accepts drops at all.
    fn drop_overlay(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let text = if self.project.is_some() {
            "drop to open file or add track"
        } else {
            "drop to open"
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop_overlay"),
        ));
        let screen = ctx.content_rect();
        painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
        painter.text(
            screen.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(22.0),
            egui::Color32::WHITE,
        );
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
                ui.vertical_centered(|ui| {
                    ui.label(format!("{path}\n\noverwrite it?"));
                });
                ui.add_space(8.0);
                // `vertical_centered` won't center a nested horizontal (it fills
                // the width), so pad the button row by half its slack ourselves.
                ui.horizontal(|ui| {
                    let font = egui::TextStyle::Button.resolve(ui.style());
                    let pad = ui.spacing().button_padding.x * 2.0;
                    let gap = ui.spacing().item_spacing.x;
                    let width = |ui: &egui::Ui, s: &str| {
                        ui.painter()
                            .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE)
                            .rect
                            .width()
                            + pad
                    };
                    let total = width(ui, "overwrite") + width(ui, "cancel") + gap;
                    ui.add_space(((ui.available_width() - total) * 0.5).max(0.0));
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

/// eframe drops the app when the window closes; that's when this session's
/// preview clips go. Best-effort: a clip still open in a player stays locked on
/// Windows and is left for the next startup sweep (`preview::sweep_temp`).
impl Drop for InterlaceApp {
    fn drop(&mut self) {
        for clip in &self.preview_clips {
            let _ = std::fs::remove_file(clip);
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

/// Turn a spawn result into the initial `RunState`: a live `Running` on success,
/// or a `Done` carrying the launch error. Shared by the model-driven and
/// escape-hatch run paths so their starting state can't drift apart.
/// `open_on_success` is the preview clip to open when the run finishes.
fn run_state_from(result: Result<Receiver<RunUpdate>, String>, open_on_success: Option<PathBuf>) -> RunState {
    match result {
        Ok(rx) => RunState::Running {
            rx,
            fraction: 0.0,
            line: "starting…".into(),
            open_on_success,
        },
        Err(e) => RunState::Done { ok: false, line: e },
    }
}

fn progress_line(p: &run::Progress) -> String {
    p.fraction
        .map(|f| format!("{:.0}%", f * 100.0))
        .unwrap_or_else(|| "—".into())
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

/// A path's final component as an owned, lossy `String`, or `None` when the path
/// has no file name (ends in `..`, or is empty). Callers supply their own fallback
/// for the `None` case — a full-path display, a synthetic `input N` label, etc.
pub(super) fn file_name_lossy(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned())
}

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
/// A fixed 22×22 close button whose `×` is painted at the exact rect center
/// (egui's `Button` can't perfectly center a lone glyph). The fixed footprint
/// also stops hover expansion from reflowing the header. Shared by the
/// inspector and the cue panel headers. Returns `true` on click.
pub(super) fn close_button(ui: &mut egui::Ui, hover: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let resp = resp.on_hover_text(hover);
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(4), v.weak_bg_fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(16.0),
            v.fg_stroke.color,
        );
    }
    resp.clicked()
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

/// Extensions treated as a single external track rather than a container. Shared
/// by the "add track" picker filter and drag-and-drop routing, so what the picker
/// offers and what a drop embeds can't drift apart.
const TRACK_EXTENSIONS: &[&str] = &[
    "srt", "ass", "ssa", "vtt", "sup", "sub", // subtitles
    "flac", "aac", "ac3", "eac3", "dts", "mp3", "opus", "ogg", "wav", "m4a", "thd",
    "mka", // audio
];

/// Whether `path` looks like an embeddable track (by extension, case-insensitive).
/// Anything else — including a file with no extension — is treated as a container.
fn is_track_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| TRACK_EXTENSIONS.iter().any(|t| t.eq_ignore_ascii_case(e)))
}

/// Order a multi-file drop for processing: the first container (if any) first, so
/// the tracks have a project to embed into, then every track in drop order.
/// Further containers come back as `skipped` — only one file can be the project,
/// and loading a second would silently replace the first.
fn order_drops(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (tracks, mut containers): (Vec<PathBuf>, Vec<PathBuf>) =
        paths.into_iter().partition(|p| is_track_file(p));
    let mut ordered = Vec::with_capacity(tracks.len() + 1);
    if !containers.is_empty() {
        ordered.push(containers.remove(0));
    }
    ordered.extend(tracks);
    (ordered, containers)
}

/// Open a native file picker for an embeddable track. Filtered to audio and
/// subtitle files only — you embed one external track, not another container.
pub(super) fn pick_track_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Audio or subtitle", TRACK_EXTENSIONS)
        .pick_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Input, Meta, Source};

    fn demo_project() -> Project {
        let mk = |input, index, kind, codec: &str, meta| {
            OutStream::new(
                Source { input, index, kind, codec: codec.into(), bitrate_kbps: None },
                meta,
                Encode::Copy,
            )
        };
        Project {
            inputs: vec![Input::new(PathBuf::from("movie.mkv"))],
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
            title: None,
            output: PathBuf::from("movie.remux.mkv"),
            duration_secs: Some(120.0),
        }
    }

    fn app_with_demo() -> InterlaceApp {
        let mut app = InterlaceApp::empty();
        app.project = Some(demo_project());
        app.selection.set_single(1);
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
        app.add_converted_streams(&[1]);
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
        assert_eq!(app.selection.single(), Some(2));

        // Both map from the same input stream → the source is emitted twice.
        let maps: Vec<String> = p
            .to_args()
            .windows(2)
            .filter(|w| w[0] == "-map")
            .map(|w| w[1].clone())
            .collect();
        assert_eq!(maps.iter().filter(|m| *m == "0:1").count(), 2);
    }

    /// An embedded track is a second input plus one `added` stream referencing it.
    /// Deleting that stream must drop the now-orphan input, and the surviving
    /// streams must all still reference input 0.
    #[test]
    fn delete_embedded_stream_prunes_input() {
        let mut app = app_with_demo(); // 3 streams, all from input 0
        {
            let p = app.project.as_mut().unwrap();
            p.inputs.push(Input::new(PathBuf::from("subs.srt")));
            let mut embedded = OutStream::new(
                Source { input: 1, index: 0, kind: Kind::Subtitle, codec: "subrip".into(), bitrate_kbps: None },
                Meta { language: Some("eng".into()), ..Default::default() },
                Encode::Copy,
            );
            embedded.added = true;
            embedded.offset_secs = -0.2;
            p.streams.push(embedded);
        }
        let last = app.project.as_ref().unwrap().streams.len() - 1;
        app.selection.set_single(last);
        app.remove_selected();

        let p = app.project.as_ref().unwrap();
        assert_eq!(p.inputs.len(), 1, "orphan input pruned");
        assert!(p.streams.iter().all(|s| s.source.input == 0));
    }

    #[test]
    fn renders_embedded_stream_with_offset() {
        // A second input with a sync offset and its embedded stream selected —
        // exercises the inspector's SYNC OFFSET section and the "from 1" summary.
        let mut app = app_with_demo();
        {
            let p = app.project.as_mut().unwrap();
            p.inputs.push(Input::new(PathBuf::from("commentary.flac")));
            let mut embedded = OutStream::new(
                Source { input: 1, index: 0, kind: Kind::Audio, codec: "flac".into(), bitrate_kbps: None },
                Meta::default(),
                Encode::Copy,
            );
            embedded.added = true;
            embedded.offset_secs = 0.15;
            p.streams.push(embedded);
        }
        app.selection.set_single(app.project.as_ref().unwrap().streams.len() - 1);
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    #[test]
    fn delete_stream_hard_removes_added_sibling() {
        let mut app = app_with_demo(); // 3 streams, selected = 1
        app.add_converted_streams(&[1]); // now 4 streams, added sibling selected at 2
        assert!(app.project.as_ref().unwrap().streams[2].added);
        app.remove_selected(); // hard delete the synthetic stream
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
        assert_eq!(app.selection.single(), Some(0));
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
            Source { input: 0, index: 9, kind: Kind::Attachment, codec: "ttf".into(), bitrate_kbps: None },
            Meta::default(),
            Encode::Copy,
        );
        app.project.as_mut().unwrap().streams.push(attach);
        app.selection.set_single(app.project.as_ref().unwrap().streams.len() - 1);
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

    /// Toggling the command panel must not leave egui painting a frame of red
    /// id-clash outlines: the transition frame should force a clean settle-pass
    /// (a discard) exactly like changing central content does.
    #[test]
    fn toggling_command_panel_discards_the_unsettled_frame() {
        let mut app = app_with_demo();
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(780.0, 720.0));
        let input = || egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        // Count shapes drawn in egui's pure-red error color — the id-clash outlines.
        let red = egui::Color32::from_rgb(255, 0, 0);
        fn count_red(shape: &egui::epaint::Shape, red: egui::Color32) -> usize {
            use egui::epaint::Shape;
            match shape {
                Shape::Rect(r) if r.stroke.color == red || r.fill == red => 1,
                Shape::LineSegment { stroke, .. } if stroke.color == red => 1,
                Shape::Circle(c) if c.stroke.color == red || c.fill == red => 1,
                Shape::Vec(v) => v.iter().map(|s| count_red(s, red)).sum(),
                _ => 0,
            }
        }
        let reds = |app: &mut InterlaceApp| {
            let out = ctx.run_ui(input(), |ui| app.render(ui));
            out.shapes.iter().map(|s| count_red(&s.shape, red)).sum::<usize>()
        };

        assert_eq!(reds(&mut app), 0); // steady state is clean
        app.show_command = false;
        assert_eq!(reds(&mut app), 0, "hiding the panel must not flash id-clash outlines");
        app.show_command = true;
        assert_eq!(reds(&mut app), 0, "showing the panel must not flash id-clash outlines");
    }

    /// A very long source-file name must not widen the top row past the window:
    /// the chip truncates into the space left after the action buttons. We can't
    /// assert pixel widths cheaply here, but rendering at the true minimum window
    /// size exercises the constrained-width layout path without panicking.
    #[test]
    fn renders_long_input_name_at_min_window() {
        let mut app = app_with_demo();
        app.project.as_mut().unwrap().inputs[0].path =
            PathBuf::from("2013.Scary.Movie.5.1920x1080.BDRip.x264.DTS-HD.MA.mkv");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(790.0, 720.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.render(ui));
    }

    #[test]
    fn renders_with_command_panel_hidden() {
        // Toggling the command panel off drops the bottom panel entirely; the
        // rest of the layout must still render (and compatibility findings stay).
        let mut app = app_with_demo();
        app.show_command = false;
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    /// Push an embedded subtitle (input 1, `added`) onto the demo project and
    /// return its row index.
    fn push_embedded_sub(app: &mut InterlaceApp) -> usize {
        let p = app.project.as_mut().unwrap();
        p.inputs.push(Input::new(PathBuf::from("subs.srt")));
        let mut embedded = OutStream::new(
            Source { input: 1, index: 0, kind: Kind::Subtitle, codec: "subrip".into(), bitrate_kbps: None },
            Meta::default(),
            Encode::Copy,
        );
        embedded.added = true;
        p.streams.push(embedded);
        p.streams.len() - 1
    }

    /// Two loaded cues on the embedded subtitle at `row`, as an open viewer.
    fn open_ready_cues(app: &mut InterlaceApp) {
        let cues = vec![
            crate::subs::Cue { start_secs: 1.0, end_secs: 2.0, text: "Hello\nthere".into() },
            crate::subs::Cue { start_secs: 3.0, end_secs: 4.0, text: "Bye".into() },
        ];
        app.cues = Some(cues::CueView::ready(PathBuf::from("subs.srt"), 0, cues));
    }

    /// The cue window renders its rows (with an offset applied) and stays open
    /// across frames while its stream is in the project.
    #[test]
    fn renders_cue_window_for_embedded_subtitle() {
        let mut app = app_with_demo();
        let row = push_embedded_sub(&mut app);
        app.project.as_mut().unwrap().streams[row].offset_secs = 1.5;
        app.selection.set_single(row);
        open_ready_cues(&mut app);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
        }
        assert!(app.cues.as_ref().is_some_and(|v| v.open));
    }

    /// The preview is offered only for a loaded, model-driven project with a
    /// readable start time.
    #[test]
    fn preview_blocker_covers_each_reason() {
        assert!(InterlaceApp::empty().preview_blocker().is_some(), "no project");

        let mut app = app_with_demo();
        assert_eq!(app.preview_blocker(), None);

        app.preview_start = "1:75".into();
        assert!(app.preview_blocker().is_some(), "bad start time");
        app.preview_start = "12:30".into();

        app.command_edit = Some("ffmpeg -i in.mkv out.mkv".into());
        assert!(app.preview_blocker().is_some(), "edited command");
        app.command_edit = None;

        let (_tx, rx) = std::sync::mpsc::channel();
        app.run_state = run_state_from(Ok(rx), None);
        assert!(app.preview_blocker().is_some(), "a run in progress");
    }

    /// Deleting the embedded track the viewer shows drops the viewer with it.
    #[test]
    fn cue_view_dropped_when_its_stream_is_deleted() {
        let mut app = app_with_demo();
        let row = push_embedded_sub(&mut app);
        open_ready_cues(&mut app);
        app.selection.set_single(row);
        app.remove_selected();
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
        assert!(app.cues.is_none());
    }

    /// An image-based subtitle selected renders the inspector's disabled
    /// "view cues…" row without panicking.
    #[test]
    fn renders_bitmap_subtitle_selection() {
        let mut app = app_with_demo();
        app.project.as_mut().unwrap().streams[2].source.codec = "hdmv_pgs_subtitle".into();
        app.selection.set_single(2);
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    /// Removing a mixed selection soft-removes the originals, hard-deletes the
    /// synthetic stream (pruning its now-orphan input), and Restore undoes the rest.
    #[test]
    fn remove_selected_soft_removes_originals_and_deletes_added() {
        let mut app = app_with_demo();
        let embedded = push_embedded_sub(&mut app); // row 3
        app.selection.set_many([1, 2, embedded]);
        app.remove_selected();

        let p = app.project.as_ref().unwrap();
        assert_eq!(p.streams.len(), 3, "embedded stream deleted outright");
        assert_eq!(p.inputs.len(), 1, "its orphan input pruned");
        assert!(p.streams[1].removed && p.streams[2].removed);
        assert!(!p.streams[0].removed);
        assert_eq!(app.selection.to_vec(), [1, 2]);

        app.restore_selected();
        assert!(app.project.as_ref().unwrap().streams.iter().all(|s| !s.removed));
    }

    /// The Delete key goes through the same path as the remove button.
    #[test]
    fn delete_key_removes_selection() {
        let mut app = app_with_demo();
        app.selection.set_many([0, 2]);
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.render(ui));
        let removed: Vec<bool> = app.project.as_ref().unwrap().streams.iter().map(|s| s.removed).collect();
        assert_eq!(removed, [true, false, true]);
    }

    #[test]
    fn add_converted_streams_inserts_a_sibling_after_each() {
        let mut app = app_with_demo();
        // Make row 2 audio too so both selected rows are convertible.
        app.project.as_mut().unwrap().streams[2].source.kind = Kind::Audio;
        app.add_converted_streams(&[1, 2]);

        let p = app.project.as_ref().unwrap();
        let added: Vec<bool> = p.streams.iter().map(|s| s.added).collect();
        assert_eq!(added, [false, false, true, false, true]);
        assert_eq!(p.streams[2].source.index, 1);
        assert_eq!(p.streams[4].source.index, 2);
        assert_eq!(app.selection.to_vec(), [2, 4], "the new siblings are selected");
    }

    #[test]
    fn is_track_file_matches_track_extensions_case_insensitively() {
        assert!(is_track_file(Path::new("subs.SRT")));
        assert!(is_track_file(Path::new(r"C:\media\commentary.flac")));
        assert!(!is_track_file(Path::new("movie.mkv")));
        assert!(!is_track_file(Path::new("clip.mp4")));
        assert!(!is_track_file(Path::new("no_extension")));
    }

    /// The container goes first so the tracks have something to embed into;
    /// extra containers are skipped rather than replacing the first.
    #[test]
    fn order_drops_puts_first_container_first_and_skips_the_rest() {
        let paths = ["eng.srt", "movie.mkv", "other.mkv", "c.flac"].map(PathBuf::from).to_vec();
        let (ordered, skipped) = order_drops(paths);
        assert_eq!(ordered, ["movie.mkv", "eng.srt", "c.flac"].map(PathBuf::from));
        assert_eq!(skipped, [PathBuf::from("other.mkv")]);

        // Tracks only: drop order is kept, nothing skipped.
        let (ordered, skipped) = order_drops(["b.ass", "a.aac"].map(PathBuf::from).to_vec());
        assert_eq!(ordered, ["b.ass", "a.aac"].map(PathBuf::from));
        assert!(skipped.is_empty());
    }

    #[test]
    fn open_dropped_empty_is_a_noop() {
        let mut app = app_with_demo();
        app.error = Some("earlier".into());
        app.open_dropped(Vec::new());
        assert_eq!(app.error.as_deref(), Some("earlier"));
        assert_eq!(app.project.as_ref().unwrap().streams.len(), 3);
    }

    /// Hovering files shows the drop overlay, with or without a project loaded.
    #[test]
    fn renders_drop_overlay_while_hovering_files() {
        let input = || egui::RawInput {
            hovered_files: vec![egui::HoveredFile {
                path: Some(PathBuf::from("movie.mkv")),
                ..Default::default()
            }],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(input(), |ui| app_with_demo().render(ui));
        let _ = ctx.run_ui(input(), |ui| InterlaceApp::empty().render(ui));
    }

    #[test]
    fn reorder_collapses_multi_selection_to_moved_row() {
        let mut app = app_with_demo();
        app.selection.select_all(3);
        app.reorder(0, 3); // video to the bottom
        assert_eq!(app.selection.single(), Some(2));
    }

    /// A mixed-kind multi-selection renders the batch inspector (mixed tags,
    /// indeterminate flags, extract disabled) without panicking.
    #[test]
    fn renders_mixed_multi_selection() {
        let mut app = app_with_demo();
        app.selection.select_all(3);
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));

        // And an all-audio selection that is partly converting, which shows the
        // mixed conversion controls.
        let mut app = app_with_demo();
        app.add_converted_streams(&[1]);
        app.selection.set_many([1, 2]);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }
}
