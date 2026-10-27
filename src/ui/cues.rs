//! The subtitle cue viewer: a panel docked under the inspector that lists one
//! text subtitle stream's cues (time + text), for checking a translation
//! without exporting.
//!
//! It fills the side panel's otherwise empty height below the inspector, with
//! its own scrolling. It stays on the stream it was opened for, with that
//! stream named in its header, rather than following the selection: loading
//! means ffmpeg reading the whole file, too slow to repeat on every click.
//! The cues are loaded once per stream on a background thread
//! (`subs::read_cues`) and polled each frame, like a run.
//!
//! A view is keyed by its input's *path* and the stream's source index, not by
//! row or input index: rows move on reorder and input indices shift when an
//! embedded track is pruned, but the path/index pair keeps naming the same
//! stream. Displayed times add that stream's current sync offset, so they match
//! what the output will contain and follow the offset live as it's edited.

use super::InterlaceApp;
use crate::model::Project;
use crate::subs::{self, Cue};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

/// Where a view's cues are in their load.
pub(crate) enum CueLoad {
    /// ffmpeg is still converting the stream; the result arrives on `rx`.
    Loading(Receiver<Result<Vec<Cue>, String>>),
    Ready(Vec<Cue>),
    Failed(String),
}

/// The open (or last opened) cue viewer.
pub(crate) struct CueView {
    /// Path of the input the stream belongs to.
    pub(crate) input: PathBuf,
    /// The stream's absolute index within that input.
    pub(crate) stream_index: usize,
    /// Window title suffix naming the stream (title, language, codec).
    label: String,
    pub(crate) load: CueLoad,
    /// Case-insensitive text filter over the cues.
    filter: String,
    /// Whether the window is shown; closing it keeps the loaded cues for reopening.
    pub(crate) open: bool,
    /// The clicked cue (index into the loaded cues).
    pub(crate) selected: Option<usize>,
}

impl CueView {
    /// Whether this view shows source stream `stream_index` of the input at `input`.
    fn is_for(&self, input: &Path, stream_index: usize) -> bool {
        self.input == input && self.stream_index == stream_index
    }

    /// Whether the project still contains the stream this view shows.
    fn still_in(&self, project: &Project) -> bool {
        project
            .streams
            .iter()
            .any(|s| self.is_for(&project.inputs[s.source.input].path, s.source.index))
    }

    /// The current sync offset of this view's stream, in seconds.
    fn offset(&self, project: &Project) -> f64 {
        project
            .streams
            .iter()
            .find(|s| self.is_for(&project.inputs[s.source.input].path, s.source.index))
            .map_or(0.0, |s| s.offset_secs)
    }

    /// Take the load result if it has arrived. Returns whether it is still loading.
    fn poll(&mut self) -> bool {
        let CueLoad::Loading(rx) = &self.load else { return false };
        match rx.try_recv() {
            Ok(Ok(cues)) => self.load = CueLoad::Ready(cues),
            Ok(Err(e)) => self.load = CueLoad::Failed(e),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => self.load = CueLoad::Failed("the cue reader stopped unexpectedly".into()),
        }
        false
    }
}

#[cfg(test)]
impl CueView {
    /// An open view whose cues have already loaded, for driving the window in tests.
    pub(crate) fn ready(input: PathBuf, stream_index: usize, cues: Vec<Cue>) -> Self {
        Self {
            input,
            stream_index,
            label: "test".into(),
            load: CueLoad::Ready(cues),
            filter: String::new(),
            open: true,
            selected: None,
        }
    }
}

impl InterlaceApp {
    /// Open the cue viewer on `streams[idx]`. Reopening the stream already loaded
    /// just shows the window again; a failed load or another stream reloads.
    pub(crate) fn open_cues(&mut self, idx: usize) {
        let Some(project) = &self.project else { return };
        let Some(stream) = project.streams.get(idx) else { return };
        let input = project.inputs[stream.source.input].path.clone();
        if let Some(view) = &mut self.cues
            && view.is_for(&input, stream.source.index)
            && !matches!(view.load, CueLoad::Failed(_))
        {
            view.open = true;
            return;
        }
        let label = [
            stream.meta.title.clone(),
            stream.meta.language.clone(),
            Some(stream.source.codec.clone()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let rx = subs::read_cues(&self.ffmpeg, &input, stream.source.index);
        self.cues = Some(CueView {
            input,
            stream_index: stream.source.index,
            label,
            load: CueLoad::Loading(rx),
            filter: String::new(),
            open: true,
            selected: None,
        });
    }
}

/// Per-frame upkeep, whether or not the panel is shown: take a finished load,
/// and drop the view once its stream is no longer in the project (another file
/// loaded, or the embedded track deleted).
pub(super) fn poll(ctx: &egui::Context, app: &mut InterlaceApp) {
    let Some(project) = &app.project else {
        app.cues = None;
        return;
    };
    let Some(view) = &mut app.cues else { return };
    if !view.still_in(project) {
        app.cues = None;
        return;
    }
    if view.poll() {
        ctx.request_repaint();
    }
}

/// Whether the cue panel should be docked under the inspector this frame.
pub(super) fn is_open(app: &InterlaceApp) -> bool {
    app.cues.as_ref().is_some_and(|v| v.open)
}

/// The cue panel, docked under the inspector: a header naming the stream with a
/// close button, then the list filling the rest of the side panel's height.
pub(super) fn panel(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let Some(project) = &app.project else { return };
    let Some(view) = &mut app.cues else { return };
    let offset = view.offset(project);
    let mut close = false;
    let mut clicked: Option<usize> = None;
    super::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = super::close_button(ui, "Close cues");
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let title = egui::RichText::new(format!("SUBTITLE CUES · {}", view.label))
                        .small()
                        .strong()
                        .color(egui::Color32::from_gray(150));
                    ui.add(egui::Label::new(title).truncate());
                });
            });
        });
        ui.add_space(4.0);
        match &view.load {
            CueLoad::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("reading subtitles…");
                });
            }
            CueLoad::Failed(e) => {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("⚠ {e}"));
            }
            CueLoad::Ready(cues) => clicked = cue_list(ui, cues, offset, &mut view.filter, view.selected),
        }
    });
    if close {
        view.open = false;
    }

    // A clicked cue becomes the preview start, with a short lead-in so the line
    // isn't already on screen when the clip begins.
    if let (Some(i), CueLoad::Ready(cues)) = (clicked, &view.load) {
        view.selected = Some(i);
        app.preview_start = subs::format_time((cues[i].start_secs + offset - CUE_LEAD_IN).max(0.0));
    }
}

/// Seconds of lead-in before a clicked cue when it sets the preview start.
const CUE_LEAD_IN: f64 = 2.0;

/// The filter box, a summary line, and the virtualized cue rows. Returns the
/// index of the cue clicked this frame, if any.
fn cue_list(ui: &mut egui::Ui, cues: &[Cue], offset: f64, filter: &mut String, selected: Option<usize>) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("filter text…")
                .desired_width(220.0),
        );
        if !filter.is_empty() && ui.small_button("×").on_hover_text("Clear filter").clicked() {
            filter.clear();
        }
    });
    let needle = filter.to_lowercase();
    let shown: Vec<usize> = (0..cues.len())
        .filter(|&i| needle.is_empty() || cues[i].text.to_lowercase().contains(&needle))
        .collect();
    let mut summary = format!("{} of {} cues", shown.len(), cues.len());
    if offset != 0.0 {
        summary.push_str(&format!(" · times include the {offset:+.3} s sync offset"));
    }
    ui.weak(summary);
    ui.weak("Click a line to start the preview clip just before it.");
    ui.add_space(4.0);
    ui.separator();

    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    // Fill what's left of the side panel, minus the card's bottom margin, so the
    // card ends at the panel's edge instead of overflowing it.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height((ui.available_height() - 14.0).max(row_height * 4.0))
        .show_rows(ui, row_height, shown.len(), |ui, range| {
            for &i in &shown[range] {
                if cue_row(ui, &cues[i], offset, row_height, selected == Some(i)) {
                    clicked = Some(i);
                }
            }
        });
    clicked
}

/// One clickable cue row: the offset start time, then the text on one line
/// (line breaks shown as ` / `), truncated to the width with the full text on
/// hover. Returns whether it was clicked.
fn cue_row(ui: &mut egui::Ui, cue: &Cue, offset: f64, height: f32, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
    let one_line = cue.text.replace('\n', " / ");
    let resp = resp.on_hover_text(&cue.text);
    if !ui.is_rect_visible(rect) {
        return resp.clicked();
    }
    let visuals = ui.style().interact_selectable(&resp, selected);
    if selected || resp.hovered() {
        ui.painter().rect_filled(rect, egui::CornerRadius::same(3), visuals.weak_bg_fill);
    }
    let painter = ui.painter();
    let time = painter.layout_no_wrap(
        subs::format_time(cue.start_secs + offset),
        egui::FontId::monospace(12.0),
        ui.visuals().weak_text_color(),
    );
    let time_pos = egui::pos2(rect.left() + 4.0, rect.center().y - time.size().y / 2.0);
    let text_left = time_pos.x + time.size().x + 12.0;
    painter.galley(time_pos, time, egui::Color32::PLACEHOLDER);

    let mut job = egui::text::LayoutJob::single_section(
        one_line,
        egui::TextFormat {
            font_id: egui::TextStyle::Body.resolve(ui.style()),
            color: visuals.text_color(),
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - 4.0 - text_left).max(0.0));
    let text = painter.layout_job(job);
    let text_pos = egui::pos2(text_left, rect.center().y - text.size().y / 2.0);
    painter.galley(text_pos, text, egui::Color32::PLACEHOLDER);
    resp.clicked()
}
