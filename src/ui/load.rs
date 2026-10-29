//! Opening files without freezing the window.
//!
//! ffprobe takes anywhere from a fraction of a second to several (a network
//! share, a cold disk). Run on the UI thread, that froze the whole window, so
//! every way in (the pickers, a drop, the file named on the command line) goes
//! through one `Loading` job instead: a plain thread probes the files, like
//! `run.rs` and `subs.rs` do, and `poll` applies all the results on the UI
//! thread at once. Only the UI thread ever touches the model.
//!
//! One load at a time. While it's in flight, drops are refused, Run and
//! Preview wait, and further picks are ignored. A quick load shows nothing; one
//! still going after `SHOW_AFTER` gets an overlay over the main column naming
//! what's being read, with Cancel. Cancel kills the running ffprobe (a slow
//! network share shouldn't leave one behind), keeps the worker from starting
//! the rest, and forgets the job; whatever the worker still sends goes into a
//! closed channel and is dropped. Closing the app cancels a load the same way.

use super::{InterlaceApp, file_name_lossy, stream_id, theme, widgets};
use crate::model::Project;
use crate::probe::{self, ProbeCancel, ProbeOutput};
use egui::{CornerRadius, RichText};
use egui_phosphor::regular as icon;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

/// How long a load runs before its overlay appears, so a quick probe doesn't
/// flash the overlay for a frame or two.
const SHOW_AFTER: Duration = Duration::from_millis(200);

/// What a load opens, and how it's recorded.
pub(crate) struct LoadRequest {
    /// The file that becomes the main file, replacing any open one.
    pub(crate) video: Option<PathBuf>,
    /// Track files to insert, in order, after the video when there is one.
    pub(crate) tracks: Vec<PathBuf>,
    /// Whether the load is an undo step with a toast (drops), or a plain open
    /// (the pickers and the command line), as before loads went async.
    pub(crate) undoable: bool,
}

/// The worker's probe results, in request order.
pub(crate) struct Probed {
    /// The video's report, when the request has a video.
    video: Option<Result<ProbeOutput, String>>,
    /// One report per track; empty when the video failed, since the tracks
    /// would have nothing to go into.
    tracks: Vec<Result<ProbeOutput, String>>,
}

/// A load in flight.
pub(crate) struct Loading {
    /// What it will do with the results.
    request: LoadRequest,
    /// When it started, to hold the overlay back for `SHOW_AFTER`.
    started: Instant,
    /// The worker's single message.
    rx: Receiver<Probed>,
    /// Kills the worker's ffprobe and stops it starting another.
    cancel: ProbeCancel,
}

impl InterlaceApp {
    /// Probe `request`'s files on a worker thread; `poll` applies them when the
    /// worker is done. Ignored while another load runs, or with nothing to load.
    pub(crate) fn start_load(&mut self, request: LoadRequest) {
        if self.loading.is_some() || (request.video.is_none() && request.tracks.is_empty()) {
            return;
        }
        let ffprobe = self.ffprobe.clone();
        let (video, tracks) = (request.video.clone(), request.tracks.clone());
        let (tx, rx) = channel();
        let cancel = ProbeCancel::default();
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            let probe = |file: &Path| probe::probe_cancellable(&ffprobe, file, &worker_cancel);
            let video = video.map(|v| probe(&v));
            let tracks = match video {
                Some(Err(_)) => Vec::new(),
                _ => tracks.iter().map(|t| probe(t)).collect(),
            };
            // Fails only when the load was cancelled, and then nobody's listening.
            let _ = tx.send(Probed { video, tracks });
        });
        self.loading = Some(Loading { request, started: Instant::now(), rx, cancel });
    }

    /// Stop the load in flight: kill its ffprobe and forget it. Its files stay
    /// unopened.
    pub(crate) fn cancel_load(&mut self) {
        if let Some(loading) = self.loading.take() {
            loading.cancel.cancel();
        }
    }

    /// Apply a finished load: open the video (if any), then insert each track
    /// after the last stream of its kind, selecting the first new row. Track
    /// failures are collected rather than stopping at the first; a video that
    /// fails leaves the open project alone and inserts nothing.
    pub(crate) fn apply_load(&mut self, request: LoadRequest, probed: Probed) {
        let before = self.snapshot();
        let replaced = self.project.is_some() && request.video.is_some();
        if let (Some(video), Some(result)) = (&request.video, probed.video) {
            match result {
                Ok(p) => self.open_project(Project::from_probe(video, p)),
                Err(e) => {
                    // Inserting the tracks into the old project would surprise
                    // more than help.
                    self.error = Some(e);
                    return;
                }
            }
        }
        let mut errors = Vec::new();
        let mut added = Vec::new();
        for (track, result) in request.tracks.iter().zip(probed.tracks) {
            // The main file can be closed while its tracks are still probing.
            let Some(project) = self.project.as_mut() else {
                errors.push(format!("No file is open to add {} to.", name(track)));
                continue;
            };
            match result.and_then(|p| project.append_probed(track, p)) {
                Ok(_) => added.push(track.clone()),
                Err(e) => errors.push(e),
            }
        }
        // Inserting shifts rows, so find the first new one by its file.
        let first = self.project.as_ref().and_then(|p| {
            let path = added.first()?;
            p.streams.iter().position(|s| &p.inputs[s.source.input].path == path)
        });
        if let Some(row) = first {
            self.selection.set_single(row);
        }
        self.error = (!errors.is_empty()).then(|| errors.join("\n"));

        if !request.undoable {
            return;
        }
        let text = match (&request.video, added.as_slice(), first) {
            (Some(v), [], _) if replaced => format!("Replaced the main file with {}", name(v)),
            (Some(v), [], _) => format!("Opened {}", name(v)),
            (Some(v), tracks, _) => format!("Opened {} and added {} track{}", name(v), tracks.len(), plural(tracks.len())),
            (None, [one], Some(row)) => {
                let id = self.project.as_ref().map(|p| stream_id(p, row)).unwrap_or_default();
                format!("Added {id} from {}", name(one))
            }
            (None, tracks, _) if !tracks.is_empty() => format!("Added {} tracks", tracks.len()),
            _ => return, // nothing changed
        };
        self.push_undo(before, text);
    }
}

/// Apply the load's results once the worker sends them. Runs every frame, and
/// keeps frames coming while a load is in flight (egui otherwise sleeps until
/// input, and the result would sit unread).
pub(super) fn poll(ctx: &egui::Context, app: &mut InterlaceApp) {
    let Some(loading) = &app.loading else { return };
    ctx.request_repaint();
    let probed = match loading.rx.try_recv() {
        Ok(probed) => probed,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => {
            app.loading = None;
            app.error = Some("Reading the file stopped unexpectedly.".into());
            return;
        }
    };
    if let Some(loading) = app.loading.take() {
        app.apply_load(loading.request, probed);
    }
}

/// The overlay over the main column once a load has run past `SHOW_AFTER`: a
/// spinner, what's being read, and Cancel (or Esc). It also takes the clicks
/// meant for the table under it, which the load is about to change.
pub(super) fn overlay(ctx: &egui::Context, app: &mut InterlaceApp) {
    let Some(loading) = &app.loading else { return };
    if loading.started.elapsed() < SHOW_AFTER {
        return;
    }
    ctx.set_cursor_icon(egui::CursorIcon::Progress);
    let title = title(&loading.request);
    let area = if app.drop_area.is_positive() { app.drop_area } else { ctx.content_rect() };
    let mut cancel = false;
    egui::Area::new(egui::Id::new("load_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(area.min)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::tint(theme::BG, 0xF7))
                .corner_radius(CornerRadius::same(8))
                .show(ui, |ui| {
                    ui.set_min_size(area.size());
                    ui.set_max_size(area.size());
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        ui.set_max_width(area.width() - 48.0);
                        // Center the block: spinner, title, helper, button.
                        let block = 28.0 + 8.0 + 22.0 + 8.0 + 16.0 + 16.0 + 26.0;
                        ui.add_space(((area.height() - block) / 2.0).max(0.0));
                        ui.spacing_mut().item_spacing.y = 8.0;
                        widgets::spinner(ui, 28.0);
                        ui.add(egui::Label::new(RichText::new(title).font(theme::medium_font(17.0)).color(theme::TEXT)).truncate());
                        ui.label(RichText::new("ffprobe is reading the streams.").size(13.0).color(theme::WEAK));
                        ui.add_space(8.0);
                        cancel = ui.add(widgets::button(widgets::with_icon(icon::X, "Cancel"))).clicked();
                    });
                });
        });
    if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.cancel_load();
    }
}

/// "Opening movie.mkv", "Opening movie.mkv and 2 tracks", "Reading subs.srt"…
fn title(request: &LoadRequest) -> String {
    match (&request.video, request.tracks.as_slice()) {
        (Some(v), []) => format!("Opening {}…", name(v)),
        (Some(v), tracks) => format!("Opening {} and {} track{}…", name(v), tracks.len(), plural(tracks.len())),
        (None, [one]) => format!("Reading {}…", name(one)),
        (None, tracks) => format!("Reading {} track files…", tracks.len()),
    }
}

/// The file name of `path`, or the whole path when it has none.
fn name(path: &Path) -> String {
    file_name_lossy(path).unwrap_or_else(|| path.display().to_string())
}

/// `s` for a count other than one.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-stream video, as ffprobe would report it.
    const MOVIE: &str = r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"h264"},
        {"index":1,"codec_type":"audio","codec_name":"aac"}],"format":{"duration":"60.0"}}"#;
    /// A single-track subtitle file.
    const SUBS: &str = r#"{"streams":[{"index":0,"codec_type":"subtitle","codec_name":"subrip"}]}"#;

    /// `json` as a probe report.
    fn report(json: &str) -> ProbeOutput {
        serde_json::from_str(json).unwrap()
    }

    /// A request for `video` and `tracks`.
    fn request(video: Option<&str>, tracks: &[&str], undoable: bool) -> LoadRequest {
        LoadRequest { video: video.map(PathBuf::from), tracks: tracks.iter().map(PathBuf::from).collect(), undoable }
    }

    /// Drive `poll` until the worker's result is applied.
    fn finish(app: &mut InterlaceApp) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.loading.is_some() {
            assert!(Instant::now() < deadline, "the load never finished");
            poll(&ctx, app);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A video plus a track is one undo step that selects the new track.
    #[test]
    fn drop_of_video_and_track_is_one_undo_step() {
        let mut app = InterlaceApp::empty();
        let probed = Probed { video: Some(Ok(report(MOVIE))), tracks: vec![Ok(report(SUBS))] };
        app.apply_load(request(Some("movie.mkv"), &["subs.srt"], true), probed);
        assert_eq!(app.project.as_ref().unwrap().streams.len(), 3);
        assert_eq!(app.selection.single(), Some(2), "the inserted subtitle");
        assert!(app.toast.as_ref().is_some_and(|t| t.text == "Opened movie.mkv and added 1 track"));
        app.undo();
        assert!(app.project.is_none());
    }

    /// A picker open isn't an undo step, as before loads went async.
    #[test]
    fn picker_open_is_not_an_undo_step() {
        let mut app = InterlaceApp::empty();
        app.apply_load(request(Some("movie.mkv"), &[], false), Probed { video: Some(Ok(report(MOVIE))), tracks: vec![] });
        assert!(app.project.is_some());
        assert_eq!(app.selection.single(), Some(0));
        assert!(app.undo.is_empty() && app.toast.is_none());
    }

    /// A video that fails to probe leaves the open project as it was.
    #[test]
    fn failed_video_keeps_the_open_project() {
        let mut app = InterlaceApp::empty();
        app.apply_load(request(Some("movie.mkv"), &[], false), Probed { video: Some(Ok(report(MOVIE))), tracks: vec![] });
        let probed = Probed { video: Some(Err("ffprobe failed".into())), tracks: vec![] };
        app.apply_load(request(Some("other.mkv"), &["subs.srt"], true), probed);
        assert_eq!(app.error.as_deref(), Some("ffprobe failed"));
        assert_eq!(app.project.as_ref().unwrap().inputs[0].path, PathBuf::from("movie.mkv"));
        assert!(app.undo.is_empty());
    }

    /// Track failures are collected; the tracks that probed still go in.
    #[test]
    fn track_failures_are_collected() {
        let mut app = InterlaceApp::empty();
        app.apply_load(request(Some("movie.mkv"), &[], false), Probed { video: Some(Ok(report(MOVIE))), tracks: vec![] });
        let probed = Probed { video: None, tracks: vec![Err("bad.srt: unreadable".into()), Ok(report(SUBS))] };
        app.apply_load(request(None, &["bad.srt", "subs.srt"], true), probed);
        assert_eq!(app.error.as_deref(), Some("bad.srt: unreadable"));
        assert_eq!(app.project.as_ref().unwrap().inputs.len(), 2);
        assert!(app.toast.as_ref().is_some_and(|t| t.text.starts_with("Added S0 from subs.srt")));
    }

    /// The worker runs off the UI thread; while it does, a second load, drops,
    /// Run and Preview wait, and its failure surfaces as the error line.
    #[test]
    fn a_load_in_flight_blocks_the_rest() {
        let mut app = InterlaceApp::empty();
        app.ffprobe = "interlace-test-no-such-ffprobe".into();
        app.load_file(PathBuf::from("movie.mkv"));
        app.load_file(PathBuf::from("other.mkv")); // ignored
        assert_eq!(app.loading.as_ref().unwrap().request.video, Some(PathBuf::from("movie.mkv")));
        assert_eq!(app.drop_blocker(), Some("Wait for the file to load"));
        assert_eq!(app.run_blocker(), Some("Wait for the file to load"));
        assert_eq!(app.preview_blocker(), Some("Wait for the file to load"));
        finish(&mut app);
        assert!(app.error.as_deref().is_some_and(|e| e.contains("could not launch ffprobe")));
        assert!(app.project.is_none());
    }

    /// Past `SHOW_AFTER` the overlay draws, and Esc cancels the load.
    #[test]
    fn overlay_shows_and_esc_cancels() {
        let ctx = egui::Context::default();
        theme::install(&ctx, false);
        let mut app = InterlaceApp::empty();
        app.ffprobe = "interlace-test-no-such-ffprobe".into();
        app.load_file(PathBuf::from("movie.mkv"));
        // Swap in a channel that never answers, so the load stays in flight.
        let (_tx, rx) = channel();
        let loading = app.loading.as_mut().unwrap();
        loading.rx = rx;
        loading.started = Instant::now() - Duration::from_secs(1);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
        assert!(app.loading.is_some());

        let esc = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        let _ = ctx.run_ui(esc, |ui| app.render(ui));
        assert!(app.loading.is_none());
    }

    #[test]
    fn titles_name_what_is_read() {
        assert_eq!(title(&request(Some("m.mkv"), &[], false)), "Opening m.mkv…");
        assert_eq!(title(&request(Some("m.mkv"), &["a.srt", "b.ac3"], true)), "Opening m.mkv and 2 tracks…");
        assert_eq!(title(&request(None, &["a.srt"], false)), "Reading a.srt…");
        assert_eq!(title(&request(None, &["a.srt", "b.ac3"], true)), "Reading 2 track files…");
    }
}
