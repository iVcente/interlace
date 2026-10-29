//! The egui/eframe view layer.
//!
//! `InterlaceApp` owns the whole session state; each screen region lives in its
//! own submodule and is handed `&mut InterlaceApp` to render:
//!
//! - `sources`: the top card (input chips, + Audio / + Subtitle, panel toggles, title)
//! - `table`: the stream table card plus the validation hints under it
//! - `inspector`: the right-hand card editing the selected streams
//! - `cues`: the subtitle cue card, stacked under the inspector when open
//! - `command`: the optional, resizable command card under the table
//! - `runbar`: the bottom card (Run, output path, progress, preview row)
//!
//! Layout follows the "floating cards" design: every panel is transparent with
//! an 8 px gutter as its margin, and draws its own rounded card inside, so cards
//! never meet at a join. Colors, fonts and widget styles come from `theme`, and
//! the small shared pieces (buttons, badges, pills…) from `widgets`.
//!
//! The table selection is a set (`selection::Selection`), so the inspector and
//! the remove/restore/convert mutations below work over every selected row at
//! once. Single-stream-only actions (extract, sync offset) need exactly one.
//!
//! Media enters through the sources-card pickers or by dropping files onto the
//! window (`drop`), which plans one action per drop and reuses the picker paths
//! (`load_file` / `embed_file`). Drops and source removals are undoable (`undo`).
//!
//! Run and Preview share one ffmpeg slot (`run_state`): each disables the other,
//! and the run card reports whichever is active or just finished.

mod command;
mod cues;
mod drop;
mod inspector;
mod runbar;
mod selection;
mod sources;
mod table;
mod undo;
pub(crate) mod theme;
mod widgets;

use crate::model::{Encode, OutStream, Project};
use crate::run::{self, RunHandle, RunUpdate};
use egui::{Frame, Margin, RichText};
use egui_phosphor::regular as icon;
use selection::Selection;
use std::path::{Path, PathBuf};
use std::sync::mpsc::TryRecvError;
use std::time::Instant;

/// Window width from which the layout switches to its wide form (split table
/// columns, a 440 px inspector, the STREAM DETAILS section).
const WIDE_FROM: f32 = 1400.0;

/// What an ffmpeg job is for, which decides how its end is reported.
pub(crate) enum JobKind {
    /// The main remux, an extract or an edited command. `output` is the file it
    /// writes, when known, for the "Wrote …" line and Show in folder.
    Run { output: Option<PathBuf> },
    /// A preview clip, opened in the default player once rendered.
    Preview { clip: PathBuf, start: f64, len: f64 },
}

/// One ffmpeg job, running or finished.
pub(crate) struct Job {
    pub(crate) kind: JobKind,
    /// When it was launched, for the elapsed time and the "in 38 s" summary.
    started: Instant,
    /// Set by Cancel, so the resulting non-zero exit isn't reported as a failure.
    cancelled: bool,
}

impl Job {
    /// A job of `kind` starting now.
    fn new(kind: JobKind) -> Self {
        Self { kind, started: Instant::now(), cancelled: false }
    }

    /// Whether this is a preview clip rather than a real run.
    pub(crate) fn is_preview(&self) -> bool {
        matches!(self.kind, JobKind::Preview { .. })
    }
}

/// How a finished job ended.
pub(crate) enum Outcome {
    /// A run wrote its output: size (if it could be read) and wall time.
    Wrote { bytes: Option<u64>, secs: f64 },
    /// A preview clip rendered and was handed to the default player.
    PreviewOpened,
    /// The user pressed Cancel.
    Cancelled,
    /// ffmpeg failed or couldn't start. `message` is one line for the run card,
    /// `log` the stderr tail shown by View log.
    Failed { message: String, log: String },
}

/// The state of the shared ffmpeg slot.
pub(crate) enum RunState {
    Idle,
    Running {
        handle: RunHandle,
        job: Job,
        /// Completion in `0..=1`, when the duration is known.
        fraction: f32,
        /// Encoding speed as a multiple of realtime, once ffmpeg reports it.
        speed: Option<f64>,
    },
    Done { job: Job, outcome: Outcome },
}

impl RunState {
    /// Whether any job (run or preview) holds the ffmpeg slot.
    pub(crate) fn is_running(&self) -> bool {
        matches!(self, RunState::Running { .. })
    }

    /// Whether a real run (not a preview) is going. Only this locks the inspector.
    pub(crate) fn is_main_running(&self) -> bool {
        matches!(self, RunState::Running { job, .. } if !job.is_preview())
    }
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
    /// its card closes so reopening the same stream needn't re-read the file.
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
    /// Whether the settings window is open.
    pub(crate) show_settings: bool,
    /// Whether the command card (a resizable panel under the table) is shown.
    pub(crate) show_command: bool,
    /// `show_command` as of the last rendered frame. Adding/removing a panel
    /// doesn't trigger egui's settle-pass the way changing central content does,
    /// so on the frame this differs we force a discard to avoid a one-frame
    /// flash of id-clash outlines while the layout re-settles.
    prev_show_command: bool,
    /// Whether the View log window (the failed job's stderr) is open.
    pub(crate) show_log: bool,
    /// Whether the output path is being typed into, rather than shown as a
    /// field that keeps the end of the path visible.
    pub(crate) editing_output: bool,
    /// When a cue click last set the preview start; the START field flashes
    /// for a moment after it (see `cues::start_flash`).
    pub(crate) start_flash: Option<Instant>,
    /// Cues read this session, so reopening a stream needs no second read.
    pub(crate) cue_cache: cues::CueCache,
    /// A drop that replaces an edited file, awaiting confirmation.
    pub(crate) pending_drop: Option<drop::DropPlan>,
    /// Snapshots before each drop or source removal, newest last.
    pub(crate) undo: Vec<undo::Snapshot>,
    /// The toast announcing the last undoable change.
    pub(crate) toast: Option<undo::Toast>,
    /// The main column's area as of this frame, which the drop overlay covers.
    pub(crate) drop_area: egui::Rect,
}

impl InterlaceApp {
    /// The app as eframe starts it: Midnight theme and fonts installed, binaries
    /// detected, and preview clips from earlier sessions swept.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx, true);
        let mut app = Self::empty();
        app.recheck_binaries(); // detect ffmpeg/ffprobe up front
        crate::preview::sweep_temp(); // clips left over from earlier sessions
        app
    }

    /// A blank session with nothing probed, as tests build it.
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
            show_log: false,
            editing_output: false,
            start_flash: None,
            cue_cache: Default::default(),
            pending_drop: None,
            undo: Vec::new(),
            toast: None,
            drop_area: egui::Rect::NOTHING,
        }
    }

    /// Re-probe both binaries and cache their status (called at startup and from
    /// the settings "Re-check" button after an override path is edited).
    pub(crate) fn recheck_binaries(&mut self) {
        self.ffmpeg_status = run::version(&self.ffmpeg);
        self.ffprobe_status = run::version(&self.ffprobe);
    }

    /// The first missing binary as `(name, configured path, error)`, if any.
    pub(crate) fn missing_binary(&self) -> Option<(&'static str, &str, &str)> {
        match (&self.ffmpeg_status, &self.ffprobe_status) {
            (Err(e), _) => Some(("ffmpeg", &self.ffmpeg, e)),
            (_, Err(e)) => Some(("ffprobe", &self.ffprobe, e)),
            _ => None,
        }
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

    /// Remove input `k` (a source chip's ×), as one undo step. The main file (0)
    /// closes the whole project; any other input takes every stream it
    /// contributed with it.
    pub(crate) fn remove_input(&mut self, k: usize) {
        let Some(p) = &self.project else { return };
        let Some(input) = p.inputs.get(k) else { return };
        let name = file_name_lossy(&input.path).unwrap_or_else(|| input.path.display().to_string());
        let before = self.snapshot();
        let text = if k == 0 {
            self.project = None;
            self.selection.clear();
            self.command_edit = None;
            self.cues = None;
            format!("Closed {name}")
        } else {
            let Some(p) = &mut self.project else { return };
            let n = p.streams.iter().filter(|s| s.source.input == k).count();
            p.streams.retain(|s| s.source.input != k);
            p.prune_orphan_inputs();
            self.selection.clear();
            format!("Removed {name} and its {n} stream{}", if n == 1 { "" } else { "s" })
        };
        self.push_undo(before, text);
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

    /// Why Run can't start right now, or `None` when it can.
    pub(crate) fn run_blocker(&self) -> Option<&'static str> {
        if self.project.is_none() && self.command_edit.is_none() {
            Some("Open a file first")
        } else if self.ffmpeg_status.is_err() {
            Some("ffmpeg wasn't found; set its path in Settings")
        } else if self.run_state.is_running() {
            Some("Wait for the current job to finish")
        } else {
            None
        }
    }

    /// Called when Run is pressed: confirm overwrite if needed, else start.
    pub(crate) fn on_run_clicked(&mut self) {
        if self.run_blocker().is_some() {
            return;
        }
        // Escape hatch: an edited command runs verbatim, bypassing the model
        // (and thus the model-based overwrite pre-check — the user owns `-y`).
        if let Some(command) = self.command_edit.clone() {
            self.start_run_edited(&command);
            return;
        }
        if let Some(p) = self.project.clone() {
            self.begin_run(p);
        }
    }

    /// Stop the running job (Run's or Preview's Cancel).
    pub(crate) fn cancel_job(&mut self) {
        if let RunState::Running { handle, job, .. } = &mut self.run_state {
            job.cancelled = true;
            handle.cancel();
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
        if self.run_state.is_running() {
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
    /// The last token is taken as the output when it isn't an option.
    pub(crate) fn start_run_edited(&mut self, command: &str) {
        let args = command_args(command);
        let output = args.last().filter(|a| !a.starts_with('-')).map(PathBuf::from);
        let duration = self.project.as_ref().and_then(|p| p.duration_secs);
        let job = Job::new(JobKind::Run { output });
        self.run_state = run_state_from(run::run_raw(&self.ffmpeg, args, duration), job);
    }

    /// Launch `project` now (`overwrite` once the user confirmed it).
    fn start_run_project(&mut self, project: Project, overwrite: bool) {
        let job = Job::new(JobKind::Run { output: Some(project.output.clone()) });
        self.run_state = run_state_from(run::run(&self.ffmpeg, &project, overwrite), job);
    }

    /// Why a preview can't start right now, or `None` when it can. Drives both
    /// the button's enabled state and the run card's info line.
    pub(crate) fn preview_blocker(&self) -> Option<&'static str> {
        if self.project.is_none() {
            Some("Open a file first")
        } else if self.run_state.is_running() {
            Some("Wait for the run to finish")
        } else if self.command_edit.is_some() {
            Some("Preview follows the table, not the edited command. Reset the command first.")
        } else if crate::preview::parse_timestamp(&self.preview_start).is_none() {
            Some("Enter a start time like 00:12:30, 12:30 or 750")
        } else {
            None
        }
    }

    /// The stream a preview should test: the single selected stream when it's a
    /// live audio/subtitle track, else the track the cue card shows (so playing
    /// a cue tests that track even with something else selected). `None` lets
    /// `preview::focus` fall back to a track with an offset.
    pub(crate) fn preview_subject(&self) -> Option<usize> {
        let project = self.project.as_ref()?;
        let testable = |i: usize| {
            project.streams.get(i).is_some_and(|s| {
                !s.removed && matches!(s.source.kind, crate::model::Kind::Audio | crate::model::Kind::Subtitle)
            })
        };
        self.selection.single().filter(|&i| testable(i)).or_else(|| self.cue_row().filter(|&i| testable(i)))
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
        let clip = crate::preview::focused(project, self.preview_subject());
        let out = crate::preview::next_path();
        let args = clip.preview_args(start, self.preview_len, &out);
        self.preview_clips.push(out.clone());
        self.error = None;
        let job = Job::new(JobKind::Preview { clip: out, start, len: self.preview_len });
        self.run_state = run_state_from(run::run_preview(&self.ffmpeg, args, self.preview_len), job);
    }

    /// Drain progress updates each frame while a job is active, and settle it
    /// into a `Done` outcome when it finishes.
    fn poll_run(&mut self, ctx: &egui::Context) {
        let RunState::Running { handle, fraction, speed, .. } = &mut self.run_state else {
            return;
        };
        let mut finished: Option<Result<(), run::RunFailure>> = None;
        loop {
            match handle.rx.try_recv() {
                Ok(RunUpdate::Progress(p)) => {
                    if let Some(f) = p.fraction {
                        *fraction = f as f32;
                    }
                    if p.speed.is_some() {
                        *speed = p.speed;
                    }
                }
                Ok(RunUpdate::Finished { result }) => {
                    finished = Some(result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(Err(run::RunFailure {
                        code: None,
                        stderr_tail: "the run ended unexpectedly".into(),
                    }));
                    break;
                }
            }
        }
        // Keep animating while a run is live (egui otherwise sleeps until input).
        ctx.request_repaint();
        let Some(result) = finished else { return };
        let RunState::Running { job, .. } = std::mem::replace(&mut self.run_state, RunState::Idle) else {
            return;
        };
        let outcome = finish(&job, result);
        self.run_state = RunState::Done { job, outcome };
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

        let wide = ui.ctx().content_rect().width() >= WIDE_FROM;
        // Each panel's margin is the gutter on the sides it doesn't share with a
        // neighbor; the neighbor's margin supplies the gap on the shared side.
        let g = theme::GUTTER as i8;
        let gutter = |left, top, right, bottom| {
            Frame::NONE.fill(theme::BG).inner_margin(Margin { left, right, top, bottom })
        };

        egui::Panel::top("sources")
            .show_separator_line(false) // the gutter separates the cards
            .frame(gutter(g, g, g, 0))
            .show(ui, |ui| sources::show(ui, self, wide));

        // Toggling a panel on/off doesn't trigger egui's automatic settle-pass
        // (unlike changing central content), so the first frame after a toggle is
        // laid out unsettled and paints a flash of red id-clash outlines. Force a
        // discard on that frame so egui re-runs the pass cleanly, as it would for
        // any other layout change. `prev_show_command` updates now (state survives
        // the discard) so the re-run pass sees no change and paints normally.
        // This runs *after* the sources card, whose terminal button flips
        // `show_command` mid-frame; checked earlier, that click's own frame
        // would paint the command card unsettled.
        if self.show_command != self.prev_show_command {
            self.prev_show_command = self.show_command;
            ui.ctx().request_discard("command panel toggled");
        }

        if self.missing_binary().is_some() {
            egui::Panel::top("banner")
                .show_separator_line(false)
                .frame(gutter(g, g, g, 0))
                .show(ui, |ui| self.missing_banner(ui));
        }

        let runbar_top = egui::Panel::bottom("runbar")
            .show_separator_line(false)
            .frame(gutter(g, 0, g, g))
            .show(ui, |ui| runbar::show(ui, self))
            .response
            .rect
            .top();

        // One panel id per width mode: egui applies a panel's default size only
        // once, so this is what lets a wide window snap to 440 while each mode
        // still remembers a width the user dragged it to.
        let inspector_id = if wide { "inspector_wide" } else { "inspector" };
        let saved = quiet_resize_line(ui);
        let inspector_rect = egui::Panel::right(inspector_id)
            .resizable(true)
            .show_separator_line(false) // keep the drag, drop the visible line
            .default_size(if wide { 440.0 } else { 320.0 })
            .size_range(260.0..=460.0)
            .frame(gutter(0, g, g, g))
            .show(ui, |ui| {
                ui.set_style(saved.clone()); // the contents keep the real style
                ui.spacing_mut().item_spacing.y = 0.0;
                let height = ui.available_height();
                // With the cue card open the column holds two cards; the
                // inspector takes the top half and scrolls inside it.
                let docked = cues::is_open(self);
                let inspector_height = if docked { ((height - theme::GUTTER) / 2.0).floor() } else { height };
                inspector::show(ui, self, inspector_height, wide);
                if docked {
                    ui.add_space(theme::GUTTER);
                    let rest = ui.available_height();
                    cues::panel(ui, self, rest);
                }
            })
            .response
            .rect;
        ui.set_style(saved);
        // The panel's edge is the inspector card's left side; the handle goes in
        // the middle of the gutter, spanning the card.
        let x = inspector_rect.left() - theme::GUTTER / 2.0;
        let span = [
            egui::pos2(x, inspector_rect.top() + theme::GUTTER),
            egui::pos2(x, inspector_rect.bottom() - theme::GUTTER),
        ];
        let mut handles = vec![(inspector_id, span)];

        egui::CentralPanel::default()
            .frame(gutter(g, g, g, g))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                if self.show_command {
                    let saved = quiet_resize_line(ui);
                    let rect = egui::Panel::bottom("command")
                        .resizable(true)
                        .show_separator_line(false)
                        .default_size(if wide { 220.0 } else { 180.0 })
                        .size_range(90.0..=420.0)
                        .frame(gutter(0, g, 0, 0))
                        .show(ui, |ui| {
                            ui.set_style(saved.clone());
                            command::show_command(ui, self)
                        })
                        .response
                        .rect;
                    ui.set_style(saved);
                    // This panel's edge is the top of the gutter above the card.
                    let y = rect.top() + theme::GUTTER / 2.0;
                    handles.push(("command", [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)]));
                }
                // The drop overlay covers this area: the main column above the
                // command card, where the table (or the empty drop zone) lives.
                self.drop_area = ui.available_rect_before_wrap();
                self.error_line(ui);
                table::show(ui, self, wide);
            });
        // Drawn after every panel, or the central column's gutter fill would
        // paint over them.
        for (id, span) in handles {
            resize_handle(ui, id, span);
        }

        self.overwrite_modal(ui.ctx());
        self.settings_window(ui.ctx());
        runbar::log_window(ui.ctx(), self);
        self.replace_modal(ui.ctx());
        undo::shortcut(ui.ctx(), self);
        undo::toast(ui.ctx(), self, runbar_top);
        drop::overlay(ui.ctx(), self);
    }

    /// The missing-ffmpeg banner card: what was tried, why it failed, and a
    /// shortcut to Settings. Run stays disabled while it shows.
    fn missing_banner(&mut self, ui: &mut egui::Ui) {
        let Some((name, tried, err)) = self.missing_binary() else { return };
        let text = format!(
            "{name} not found — Tried {tried} — {err}. Set the path in Settings, or install ffmpeg and add it to PATH."
        );
        let mut open = false;
        Frame::NONE
            .fill(theme::REMOVED)
            .stroke(egui::Stroke::new(1.0, theme::ERROR_STROKE))
            .corner_radius(8)
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        open = ui
                            .add(widgets::button(widgets::with_icon(icon::GEAR_SIX, "Open settings")))
                            .clicked();
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            widgets::wrapped_with_icon(ui, icon::X_CIRCLE, theme::ERROR, &text, theme::ERROR_TEXT, 13.0, 2);
                        });
                    });
                });
            });
        if open {
            self.show_settings = true;
        }
    }

    /// The last load/drop error, if any, as a dismissable line above the table.
    fn error_line(&mut self, ui: &mut egui::Ui) {
        let Some(err) = &self.error else { return };
        let mut dismiss = false;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                dismiss = widgets::close_x(ui, 18.0, "Dismiss");
                ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                    widgets::wrapped_with_icon(ui, icon::X_CIRCLE, theme::ERROR, err, theme::ERROR, 12.5, 4);
                });
            });
        });
        ui.add_space(2.0);
        if dismiss {
            self.error = None;
        }
    }

    /// The overwrite confirmation for a run whose output already exists.
    fn overwrite_modal(&mut self, ctx: &egui::Context) {
        let Some(project) = &self.pending_run else { return };
        let path = project.output.display().to_string();

        let (mut start, mut cancel) = (false, false);
        egui::Window::new(RichText::new(widgets::with_icon(icon::WARNING, "Output file exists")).font(theme::medium_font(14.0)))
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add(egui::Label::new(RichText::new(&path).color(theme::WEAK)).wrap());
                ui.add_space(4.0);
                ui.label("Overwrite it?");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    start = ui.add(widgets::accent_button("Overwrite")).clicked();
                    cancel = ui.add(widgets::button("Cancel")).clicked();
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

    /// The settings window: editable ffmpeg/ffprobe paths, their detected
    /// version (or error), and a button to re-probe after editing.
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut recheck = false;
        egui::Window::new(RichText::new("Settings").font(theme::medium_font(14.0)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::RIGHT_TOP, [-16.0, 64.0])
            .show(ctx, |ui| {
                widgets::section_label(ui, "BINARIES");
                widgets::helper(ui, "A bare name resolves via PATH; enter a full path to override.");
                ui.add_space(4.0);
                egui::Grid::new("binaries_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("ffmpeg");
                        ui.add(egui::TextEdit::singleline(&mut self.ffmpeg).desired_width(300.0));
                        ui.end_row();
                        ui.label("");
                        binary_status(ui, &self.ffmpeg_status);
                        ui.end_row();

                        ui.label("ffprobe");
                        ui.add(egui::TextEdit::singleline(&mut self.ffprobe).desired_width(300.0));
                        ui.end_row();
                        ui.label("");
                        binary_status(ui, &self.ffprobe_status);
                        ui.end_row();
                    });
                ui.add_space(6.0);
                recheck = ui
                    .add(widgets::button(widgets::with_icon(icon::ARROW_CLOCKWISE, "Re-check")))
                    .clicked();
            });
        if recheck {
            self.recheck_binaries();
        }
        self.show_settings = open;
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
    /// @see more info in eframe's `App::ui`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
}

/// Make egui's own panel resize line invisible for the next panel shown into
/// `ui`, returning the style to restore (inside the panel, and after it).
///
/// egui paints that line in the hovered/active widget foreground color (white)
/// right on the panel's edge, which here is a card's side rather than the
/// middle of the gutter. It reads the parent `ui`'s style when painting, so
/// blanking those two strokes there hides it; `resize_handle` draws ours.
fn quiet_resize_line(ui: &mut egui::Ui) -> std::sync::Arc<egui::Style> {
    let saved = ui.style().clone();
    let widgets = &mut ui.style_mut().visuals.widgets;
    widgets.hovered.fg_stroke = egui::Stroke::NONE;
    widgets.active.fg_stroke = egui::Stroke::NONE;
    saved
}

/// The resize affordance for panel `panel_id`: a 2 px rounded line along
/// `span` (centered in the gutter), shown while the edge is hovered and in the
/// accent while it's dragged. egui's grab zone straddles the panel edge, so
/// it covers the gutter this is drawn in.
fn resize_handle(ui: &egui::Ui, panel_id: &str, span: [egui::Pos2; 2]) {
    let Some(resp) = ui.ctx().read_response(egui::Id::new(panel_id).with("__resize")) else { return };
    let color = if resp.dragged() {
        theme::ACCENT
    } else if resp.hovered() {
        theme::DIM
    } else {
        return;
    };
    let rect = egui::Rect::from_two_pos(span[0], span[1]).expand(1.0);
    ui.painter().rect_filled(rect, egui::CornerRadius::same(1), color);
}

/// How `job` ended, given ffmpeg's verdict. A successful preview is opened in
/// the default player here, so a failure to launch it is reported like any
/// other failure.
fn finish(job: &Job, result: Result<(), run::RunFailure>) -> Outcome {
    let secs = job.started.elapsed().as_secs_f64();
    match (result, &job.kind) {
        (Err(_), _) if job.cancelled => Outcome::Cancelled,
        (Err(f), _) => Outcome::Failed { message: failure_line(&f), log: f.stderr_tail },
        (Ok(()), JobKind::Preview { clip, .. }) => match run::open_in_default_app(clip) {
            Ok(()) => Outcome::PreviewOpened,
            Err(e) => Outcome::Failed { message: e.clone(), log: e },
        },
        (Ok(()), JobKind::Run { output }) => Outcome::Wrote {
            bytes: output.as_deref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()),
            secs,
        },
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
/// or a `Done` carrying the launch error. Shared by every job path so their
/// starting state can't drift apart.
fn run_state_from(result: Result<RunHandle, String>, job: Job) -> RunState {
    match result {
        Ok(handle) => RunState::Running { handle, job, fraction: 0.0, speed: None },
        Err(e) => RunState::Done {
            job,
            outcome: Outcome::Failed { message: e.clone(), log: e },
        },
    }
}

/// The run card's one-line failure: exit code plus ffmpeg's last stderr line,
/// which is almost always the actual error.
fn failure_line(f: &run::RunFailure) -> String {
    let last = f.stderr_tail.lines().last().unwrap_or("ffmpeg failed");
    match f.code {
        Some(code) => format!("ffmpeg exited with code {code}: {last}"),
        None => last.to_string(),
    }
}

/// A binary's last probe result in Settings: version, error, or "not checked".
fn binary_status(ui: &mut egui::Ui, status: &Result<String, String>) {
    ui.horizontal(|ui| {
        ui.set_max_width(300.0);
        match status {
            Ok(v) if v.is_empty() => {
                widgets::line(ui, RichText::new("not checked").color(theme::WEAK));
            }
            Ok(v) => {
                widgets::status_line(ui, icon::CHECK_CIRCLE, v, theme::SUCCESS);
            }
            Err(e) => {
                widgets::status_line(ui, icon::X_CIRCLE, e, theme::ERROR);
            }
        }
    });
}

// --- shared helpers (used by the submodules) ---------------------------------

/// A path's final component as an owned, lossy `String`, or `None` when the path
/// has no file name (ends in `..`, or is empty). Callers supply their own fallback
/// for the `None` case — a full-path display, a synthetic `input N` label, etc.
pub(super) fn file_name_lossy(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// The short id of `streams[idx]`: its kind letter plus its index among the
/// streams of that kind in output order (`A2` = the third audio row). Removed
/// rows count, so ids don't jump while a removal is only pending.
pub(super) fn stream_id(project: &Project, idx: usize) -> String {
    let kind = project.streams[idx].source.kind;
    let n = project.streams[..idx].iter().filter(|s| s.source.kind == kind).count();
    format!("{}{n}", kind.spec().to_ascii_uppercase())
}

/// `streams[idx]` named for one line: `id · lang · title`, falling back to the
/// source file name and then the codec when there's no title (`S9 · Night
/// Train.pt-BR.srt`, `A1 · jpn · Japanese Stereo`).
pub(super) fn short_name(project: &Project, idx: usize) -> String {
    let s = &project.streams[idx];
    let mut parts = vec![stream_id(project, idx)];
    if let Some(lang) = &s.meta.language {
        parts.push(lang.clone());
    }
    let name = s.meta.title.clone().or_else(|| {
        (s.source.input != 0)
            .then(|| file_name_lossy(&project.inputs[s.source.input].path))
            .flatten()
    });
    parts.push(name.unwrap_or_else(|| s.source.codec.clone()));
    parts.join(" · ")
}

/// A signed offset for display, with a real minus sign: `+0.150 s`, `−0.500 s`.
pub(super) fn format_offset(secs: f64) -> String {
    let sign = if secs < 0.0 { '−' } else { '+' };
    format!("{sign}{:.3} s", secs.abs())
}

/// Open a native file picker for media files. Returns the chosen path, if any.
pub(super) fn pick_media_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(
            "Media",
            &["mkv", "mp4", "m4v", "mov", "webm", "avi", "ts", "m2ts", "flac", "aac", "ac3", "srt", "ass", "sup"],
        )
        .pick_file()
}

/// Open a native file picker for an audio track to insert (`+ Audio`).
pub(super) fn pick_audio_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Audio", drop::AUDIO_EXTENSIONS)
        .pick_file()
}

/// Open a native file picker for a subtitle track to insert (`+ Subtitle`).
pub(super) fn pick_subtitle_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Subtitles", drop::SUBTITLE_EXTENSIONS)
        .pick_file()
}

/// Ask where to write the output, starting from the current path.
pub(super) fn pick_output_file(current: &Path) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().add_filter("Media", &["mkv", "mp4", "mov", "webm", "mka", "mks"]);
    if let Some(dir) = current.parent().filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    if let Some(name) = file_name_lossy(current) {
        dialog = dialog.set_file_name(name);
    }
    dialog.save_file()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Input, Kind, Meta, Source};

    /// A context with the app's theme and fonts, as `new` installs them (minus
    /// the system fallbacks, so tests don't depend on the machine's fonts).
    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::install(&ctx, false);
        ctx
    }

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
        let ctx = test_ctx();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    #[test]
    fn renders_empty_state() {
        let mut app = InterlaceApp::empty();
        app.error = Some("something went wrong".into());
        let ctx = test_ctx();
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
        let ctx = test_ctx();
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
        let ctx = test_ctx();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    #[test]
    fn renders_settings_panel_and_missing_binary_banner() {
        let mut app = app_with_demo();
        app.show_settings = true;
        app.ffmpeg_status = Err("not found (`ffmpeg`)".into());
        app.ffprobe_status = Ok("ffprobe version 8.1".into());
        assert!(app.missing_binary().is_some());
        let ctx = test_ctx();
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
        let ctx = test_ctx();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }

    /// Toggling the command panel must not leave egui painting a frame of red
    /// id-clash outlines: the transition frame should force a clean settle-pass
    /// (a discard) exactly like changing central content does.
    #[test]
    fn toggling_command_panel_discards_the_unsettled_frame() {
        for (w, h) in [(1000.0, 650.0), (1600.0, 900.0)] {
            for loaded in [false, true] {
                let mut app = if loaded { app_with_demo() } else { InterlaceApp::empty() };
                let reds = toggle_reds(&mut app, w, h);
                assert_eq!(reds, [0; 9], "{w}x{h}, loaded={loaded}: red outlines per frame");
            }
        }
    }

    /// Clicking the terminal button flips the card mid-frame (the sources card
    /// is drawn before the table), which must not flash outlines either.
    #[test]
    fn clicking_the_command_toggle_does_not_flash() {
        for loaded in [false, true] {
            let mut app = if loaded { app_with_demo() } else { InterlaceApp::empty() };
            let ctx = test_ctx();
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 650.0));
            // The terminal toggle sits second from the right in the sources card.
            let at = egui::pos2(921.0, 31.0);
            let frame = |events: Vec<egui::Event>| egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mut reds = Vec::new();
            for events in [
                vec![egui::Event::PointerMoved(at)],
                vec![press(true)],
                vec![press(false)],
                vec![],
                vec![],
            ] {
                let out = ctx.run_ui(frame(events), |ui| app.render(ui));
                reds.push(out.shapes.iter().map(|s| count_red(&s.shape)).sum::<usize>());
            }
            assert!(app.show_command, "the click toggled the card");
            assert_eq!(reds, [0; 5], "loaded={loaded}: red outlines per frame");
        }
    }

    /// Clicking from one stream to another swaps the inspector's sections (a
    /// video has no language row, audio has conversion…), which must not flash
    /// id-clash outlines on any frame.
    #[test]
    fn switching_streams_does_not_flash() {
        for (w, h) in [(1000.0, 650.0), (1600.0, 900.0)] {
            let mut app = app_with_demo();
            push_embedded_sub(&mut app);
            app.selection.clear();
            let ctx = test_ctx();
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h));
            let frame = |events: Vec<egui::Event>| egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut reds = Vec::new();
            // Rows sit 34 px apart below the header; click each, then back to the first.
            for row in [0, 1, 2, 3, 0, 2] {
                let at = egui::pos2(200.0, 140.0 + 34.0 * row as f32);
                let press = |pressed| egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                for events in [vec![egui::Event::PointerMoved(at)], vec![press(true)], vec![press(false)], vec![], vec![]] {
                    let out = ctx.run_ui(frame(events), |ui| app.render(ui));
                    reds.push(out.shapes.iter().map(|s| count_red(&s.shape)).sum::<usize>());
                }
                assert_eq!(app.selection.single(), Some(row), "{w}x{h}: row {row} got selected");
            }
            assert!(reds.iter().all(|&r| r == 0), "{w}x{h}: red outlines per frame {reds:?}");
        }
    }

    /// Other state changes that reshape the right column or the table don't
    /// flash outlines either: the cue card opening, a multi-selection, a
    /// removal, and a converted copy being added.
    #[test]
    fn reshaping_changes_do_not_flash() {
        let changes: Vec<Box<dyn Fn(&mut InterlaceApp)>> = vec![
            Box::new(|app| {
                app.selection.set_single(3);
                open_ready_cues(app);
            }),
            Box::new(|app| app.selection.set_many([1, 2, 3])),
            Box::new(|app| {
                app.selection.set_single(1);
                app.remove_selected();
            }),
            Box::new(|app| app.add_converted_streams(&[1])),
        ];
        for (i, change) in changes.iter().enumerate() {
            let mut app = app_with_demo();
            push_embedded_sub(&mut app);
            let ctx = test_ctx();
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 650.0));
            let input = || egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let mut reds = Vec::new();
            for f in 0..6 {
                if f == 2 {
                    change(&mut app);
                }
                let out = ctx.run_ui(input(), |ui| app.render(ui));
                reds.push(out.shapes.iter().map(|s| count_red(&s.shape)).sum::<usize>());
            }
            assert_eq!(reds, [0; 6], "change {i}: red outlines per frame");
        }
    }

    /// Shapes drawn in egui's pure-red error color: the id-clash outlines.
    fn count_red(shape: &egui::epaint::Shape) -> usize {
        use egui::epaint::Shape;
        let red = egui::Color32::from_rgb(255, 0, 0);
        match shape {
            Shape::Rect(r) if r.stroke.color == red || r.fill == red => 1,
            Shape::LineSegment { stroke, .. } if stroke.color == red => 1,
            Shape::Circle(c) if c.stroke.color == red || c.fill == red => 1,
            Shape::Vec(v) => v.iter().map(count_red).sum(),
            _ => 0,
        }
    }

    /// Red id-clash shapes per frame while toggling the command card: three
    /// steady frames, three after showing it, three after hiding it.
    fn toggle_reds(app: &mut InterlaceApp, w: f32, h: f32) -> Vec<usize> {
        let ctx = test_ctx();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h));
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

        let mut out = Vec::new();
        for show in [false, true, false] {
            app.show_command = show;
            for _ in 0..3 {
                out.push(reds(app));
            }
        }
        out
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
        let ctx = test_ctx();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(790.0, 720.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.render(ui));
    }

    /// Render one frame of `app` in a `w`×`h` window.
    fn render_at(app: &mut InterlaceApp, w: f32, h: f32) {
        let ctx = test_ctx();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.render(ui));
    }

    /// The wide layout (split columns, stream details) renders with a stream
    /// that has an offset, so every inspector section is drawn.
    #[test]
    fn renders_wide_layout() {
        let mut app = app_with_demo();
        app.project.as_mut().unwrap().streams[1].offset_secs = -0.5;
        app.show_command = true;
        render_at(&mut app, 1600.0, 900.0);
    }

    /// Each run-card state renders: running (inspector locked), done, failed
    /// (with the log window open), cancelled, and a running preview.
    #[test]
    fn renders_every_run_state() {
        let run = || Job::new(JobKind::Run { output: Some(PathBuf::from("movie.remux.mkv")) });
        let states: Vec<Box<dyn Fn() -> RunState>> = vec![
            Box::new(|| run_state_from(Ok(RunHandle::detached(std::sync::mpsc::channel().1)), run())),
            Box::new(|| RunState::Done { job: run(), outcome: Outcome::Wrote { bytes: Some(1_410_000_000), secs: 38.0 } }),
            Box::new(|| RunState::Done {
                job: run(),
                outcome: Outcome::Failed { message: "ffmpeg exited with code 1: boom".into(), log: "a\nb\nboom".into() },
            }),
            Box::new(|| RunState::Done { job: run(), outcome: Outcome::Cancelled }),
            Box::new(|| {
                let job = Job::new(JobKind::Preview { clip: PathBuf::from("clip.mkv"), start: 93.6, len: 30.0 });
                run_state_from(Ok(RunHandle::detached(std::sync::mpsc::channel().1)), job)
            }),
        ];
        for state in states {
            let mut app = app_with_demo();
            app.run_state = state();
            app.show_log = true;
            render_at(&mut app, 1000.0, 650.0);
        }
    }

    /// A real run locks the inspector and blocks a second job; a preview
    /// doesn't lock the inspector but still holds the ffmpeg slot.
    #[test]
    fn running_job_kinds_gate_run_and_inspector() {
        let mut app = app_with_demo();
        let job = Job::new(JobKind::Run { output: None });
        app.run_state = run_state_from(Ok(RunHandle::detached(std::sync::mpsc::channel().1)), job);
        assert!(app.run_state.is_main_running());
        assert!(app.run_blocker().is_some());

        let job = Job::new(JobKind::Preview { clip: PathBuf::from("c.mkv"), start: 0.0, len: 10.0 });
        app.run_state = run_state_from(Ok(RunHandle::detached(std::sync::mpsc::channel().1)), job);
        assert!(!app.run_state.is_main_running());
        assert!(app.run_blocker().is_some());
    }

    /// A finished job settles into the matching outcome; a cancelled one isn't
    /// reported as a failure.
    #[test]
    fn finish_maps_results_to_outcomes() {
        let fail = || Err(run::RunFailure { code: Some(1), stderr_tail: "x\nNo such file".into() });
        let job = Job::new(JobKind::Run { output: None });
        assert!(matches!(finish(&job, Ok(())), Outcome::Wrote { bytes: None, .. }));
        match finish(&job, fail()) {
            Outcome::Failed { message, .. } => assert_eq!(message, "ffmpeg exited with code 1: No such file"),
            _ => panic!("expected a failure"),
        }
        let mut cancelled = Job::new(JobKind::Run { output: None });
        cancelled.cancelled = true;
        assert!(matches!(finish(&cancelled, fail()), Outcome::Cancelled));
    }

    /// Stream ids count per kind in output order, and short names fall back
    /// from title to file name to codec.
    #[test]
    fn stream_ids_and_short_names() {
        let mut app = app_with_demo();
        let row = push_embedded_sub(&mut app);
        let p = app.project.as_ref().unwrap();
        assert_eq!(stream_id(p, 0), "V0");
        assert_eq!(stream_id(p, 1), "A0");
        assert_eq!(stream_id(p, row), "S1");
        assert_eq!(short_name(p, 1), "A0 · jpn · Japanese");
        assert_eq!(short_name(p, 2), "S0 · eng · subrip");
        assert_eq!(short_name(p, row), "S1 · subs.srt");
        assert_eq!(format_offset(-0.5), "−0.500 s");
    }

    /// A chip's × on a track file drops its streams and input; on the main
    /// file it closes the project.
    #[test]
    fn remove_input_drops_its_streams_or_closes() {
        let mut app = app_with_demo();
        push_embedded_sub(&mut app);
        app.remove_input(1);
        let p = app.project.as_ref().unwrap();
        assert_eq!((p.inputs.len(), p.streams.len()), (1, 3));
        app.remove_input(0);
        assert!(app.project.is_none());
    }

    #[test]
    fn renders_with_command_panel_hidden() {
        // Toggling the command panel off drops the bottom panel entirely; the
        // rest of the layout must still render (and compatibility findings stay).
        let mut app = app_with_demo();
        app.show_command = false;
        let ctx = test_ctx();
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
        let ctx = test_ctx();
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
        app.run_state = run_state_from(Ok(RunHandle::detached(rx)), Job::new(JobKind::Run { output: None }));
        assert!(app.preview_blocker().is_some(), "a run in progress");
    }

    /// The cue card's loading, failed and no-match states render, at both widths.
    #[test]
    fn renders_cue_card_states() {
        let views: Vec<Box<dyn Fn() -> cues::CueView>> = vec![
            Box::new(|| cues::CueView::loading(PathBuf::from("subs.srt"), 0)),
            Box::new(|| cues::CueView::failed(PathBuf::from("subs.srt"), 0, "Invalid data found when processing input")),
            Box::new(|| {
                let mut v = cues::CueView::ready(
                    PathBuf::from("subs.srt"),
                    0,
                    vec![crate::subs::Cue { start_secs: 1.0, end_secs: 2.0, text: "Olá".into() }],
                );
                v.set_filter("xícara");
                v
            }),
        ];
        for view in views {
            for (w, h) in [(1000.0, 650.0), (1600.0, 900.0)] {
                let mut app = app_with_demo();
                push_embedded_sub(&mut app);
                app.cues = Some(view());
                render_at(&mut app, w, h);
                assert!(app.cues.is_some(), "the card stays while its stream exists");
            }
        }
    }

    /// Preview tests the selected track, or else the cue card's track.
    #[test]
    fn preview_subject_prefers_selection_then_cue_card() {
        let mut app = app_with_demo();
        let row = push_embedded_sub(&mut app);
        open_ready_cues(&mut app);
        app.selection.set_single(0); // video: not testable
        assert_eq!(app.preview_subject(), Some(row));
        app.selection.set_single(1);
        assert_eq!(app.preview_subject(), Some(1));
    }

    /// A stream whose cues were read this session reopens from the cache.
    #[test]
    fn open_cues_uses_the_session_cache() {
        let mut app = app_with_demo();
        let row = push_embedded_sub(&mut app);
        let cue = crate::subs::Cue { start_secs: 1.0, end_secs: 2.0, text: "Hi".into() };
        app.cue_cache.insert((PathBuf::from("subs.srt"), 0), std::sync::Arc::new(vec![cue]));
        app.open_cues(row);
        assert!(matches!(app.cues.as_ref().unwrap().load, cues::CueLoad::Ready(_)));
        assert_eq!(app.cue_row(), Some(row));
    }

    /// Deleting the embedded track the viewer shows drops the viewer with it.
    #[test]
    fn cue_view_dropped_when_its_stream_is_deleted() {
        let mut app = app_with_demo();
        let row = push_embedded_sub(&mut app);
        open_ready_cues(&mut app);
        app.selection.set_single(row);
        app.remove_selected();
        let ctx = test_ctx();
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
        let ctx = test_ctx();
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
        let ctx = test_ctx();
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
        let ctx = test_ctx();
        let _ = ctx.run_ui(input(), |ui| app_with_demo().render(ui));
        let _ = ctx.run_ui(input(), |ui| InterlaceApp::empty().render(ui));
    }

    /// The overlay renders every kind of plan: replace with edits, insert one
    /// and several, video plus tracks, skipped files, and a refusal.
    #[test]
    fn renders_drop_overlay_for_each_plan() {
        let drops: [&[&str]; 5] = [
            &["movie.mkv"],
            &["subs.srt"],
            &["a.srt", "b.ac3"],
            &["other.mp4", "a.srt", "b.ac3", "notes.txt"],
            &["notes.txt"],
        ];
        for files in drops {
            let mut app = app_with_demo();
            app.project.as_mut().unwrap().streams[1].removed = true; // an edit
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 650.0)));
            input.hovered_files = files
                .iter()
                .map(|f| egui::HoveredFile { path: Some(PathBuf::from(f)), ..Default::default() })
                .collect();
            let ctx = test_ctx();
            let _ = ctx.run_ui(input, |ui| app.render(ui));
        }
    }

    /// Edits are counted per changed stream, plus a reorder; the (probed)
    /// title isn't one.
    #[test]
    fn edit_count_tallies_changes() {
        let mut app = app_with_demo();
        assert_eq!(drop::edit_count(app.project.as_ref().unwrap()), 0);
        {
            let p = app.project.as_mut().unwrap();
            p.streams[1].removed = true;
            p.streams[2].offset_secs = 0.5;
            p.title = Some("x".into());
        }
        app.reorder(2, 0);
        assert_eq!(drop::edit_count(app.project.as_ref().unwrap()), 3);
    }

    /// Removing a track file is one undo step, announced by the toast; undo
    /// brings the input, its stream and the selection back.
    #[test]
    fn remove_input_is_undoable() {
        let mut app = app_with_demo();
        let row = push_embedded_sub(&mut app);
        app.selection.set_single(row);
        app.remove_input(1);
        assert_eq!(app.project.as_ref().unwrap().inputs.len(), 1);
        assert!(app.toast.as_ref().is_some_and(|t| t.text.contains("subs.srt")));
        render_at(&mut app, 1000.0, 650.0); // draws the toast

        app.undo();
        let p = app.project.as_ref().unwrap();
        assert_eq!((p.inputs.len(), p.streams.len()), (2, 4));
        assert_eq!(app.selection.single(), Some(row));
        assert!(app.toast.is_none());

        // Closing the main file comes back too.
        app.remove_input(0);
        assert!(app.project.is_none());
        app.undo();
        assert!(app.project.is_some());
    }

    /// A drop that would discard edits waits for confirmation; refusals and
    /// empty drops do nothing.
    #[test]
    fn replacing_an_edited_file_asks_first() {
        let mut app = app_with_demo();
        app.project.as_mut().unwrap().streams[1].removed = true;
        app.open_dropped(vec![PathBuf::from("other.mkv")]);
        assert!(app.pending_drop.is_some());
        render_at(&mut app, 1000.0, 650.0); // draws the confirm window

        let mut app = InterlaceApp::empty();
        app.open_dropped(vec![PathBuf::from("subs.srt")]); // no video open: refused
        assert!(app.project.is_none() && app.pending_drop.is_none() && app.undo.is_empty());
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
        let ctx = test_ctx();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));

        // And an all-audio selection that is partly converting, which shows the
        // mixed conversion controls.
        let mut app = app_with_demo();
        app.add_converted_streams(&[1]);
        app.selection.set_many([1, 2]);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
    }
}
