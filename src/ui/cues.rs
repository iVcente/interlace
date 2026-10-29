//! The subtitle cue card: stacked under the inspector, it lists one text
//! subtitle stream's cues (time + text) for the check loop the preview is built
//! around: read the lines, click one, preview a clip, nudge the offset, preview
//! again.
//!
//! It stays on the stream it was opened for, named in its header, rather than
//! following the table selection: loading means ffmpeg reading the whole file,
//! too slow to repeat on every click. Cues load once per stream on a background
//! thread (`subs::read_cues`), polled each frame like a run, and are cached for
//! the session so reopening any stream already read is instant.
//!
//! A view is keyed by its input's *path* and the stream's source index, not by
//! row or input index: rows move on reorder and input indices shift when an
//! inserted track is pruned, but the path/index pair keeps naming the same
//! stream. The card's OFFSET stepper edits that stream's `offset_secs`, the
//! same field as the inspector's SYNC OFFSET, and displayed times add it, so
//! they match what the output will contain.
//!
//! Clicking a cue sets the preview START 2 s before it (and flashes the field);
//! its ▶ button does the same and starts the preview right away.

use super::widgets::{self, line};
use super::{InterlaceApp, stream_id, theme};
use crate::model::Project;
use crate::subs::{self, Cue};
use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Seconds of lead-in before a clicked cue when it sets the preview start.
const CUE_LEAD_IN: f64 = 2.0;
/// How long the START field flashes after a cue click sets it, in seconds.
const FLASH_SECS: f32 = 1.2;
/// Cue row height.
const ROW_HEIGHT: f32 = 24.0;

/// The session's parsed cues, by input path and source stream index.
pub(crate) type CueCache = std::collections::HashMap<(PathBuf, usize), Arc<Vec<Cue>>>;

/// Where a view's cues are in their load.
pub(crate) enum CueLoad {
    /// ffmpeg is still converting the stream; the result arrives on `rx`.
    Loading(Receiver<Result<Vec<Cue>, String>>),
    Ready(Arc<Vec<Cue>>),
    Failed(String),
}

/// The open (or last opened) cue card.
pub(crate) struct CueView {
    /// Path of the input the stream belongs to.
    pub(crate) input: PathBuf,
    /// The stream's absolute index within that input.
    pub(crate) stream_index: usize,
    pub(crate) load: CueLoad,
    /// Case- and accent-insensitive text filter over the cues.
    filter: String,
    /// Whether the card is shown; closing it keeps the loaded cues for reopening.
    pub(crate) open: bool,
    /// The clicked cue (index into the loaded cues).
    pub(crate) selected: Option<usize>,
}

impl CueView {
    /// A fresh view on `input`'s stream `stream_index` in state `load`.
    fn new(input: PathBuf, stream_index: usize, load: CueLoad) -> Self {
        Self { input, stream_index, load, filter: String::new(), open: true, selected: None }
    }

    /// Whether this view shows source stream `stream_index` of the input at `input`.
    fn is_for(&self, input: &Path, stream_index: usize) -> bool {
        self.input == input && self.stream_index == stream_index
    }

    /// The row of the stream this view shows, if it's still in the project.
    fn row(&self, project: &Project) -> Option<usize> {
        project
            .streams
            .iter()
            .position(|s| self.is_for(&project.inputs[s.source.input].path, s.source.index))
    }

    /// Take the load result if it has arrived, caching cues that loaded.
    /// Returns whether it is still loading.
    fn poll(&mut self, cache: &mut CueCache) -> bool {
        let CueLoad::Loading(rx) = &self.load else { return false };
        match rx.try_recv() {
            Ok(Ok(cues)) => {
                let cues = Arc::new(cues);
                cache.insert((self.input.clone(), self.stream_index), Arc::clone(&cues));
                self.load = CueLoad::Ready(cues);
            }
            Ok(Err(e)) => self.load = CueLoad::Failed(e),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => self.load = CueLoad::Failed("the cue reader stopped unexpectedly".into()),
        }
        false
    }
}

#[cfg(test)]
impl CueView {
    /// An open view whose cues have already loaded, for driving the card in tests.
    pub(crate) fn ready(input: PathBuf, stream_index: usize, cues: Vec<Cue>) -> Self {
        Self::new(input, stream_index, CueLoad::Ready(Arc::new(cues)))
    }

    /// An open view still waiting on its load, for rendering that state.
    pub(crate) fn loading(input: PathBuf, stream_index: usize) -> Self {
        Self::new(input, stream_index, CueLoad::Loading(std::sync::mpsc::channel().1))
    }

    /// An open view whose load failed with `error`.
    pub(crate) fn failed(input: PathBuf, stream_index: usize, error: &str) -> Self {
        Self::new(input, stream_index, CueLoad::Failed(error.into()))
    }

    /// Set the filter text, as if typed.
    pub(crate) fn set_filter(&mut self, filter: &str) {
        self.filter = filter.into();
    }
}

impl InterlaceApp {
    /// Open the cue card on `streams[idx]`. Reopening the stream it already
    /// shows just shows it again; a stream read earlier this session comes from
    /// the cache; anything else (or a failed load) reads the file.
    pub(crate) fn open_cues(&mut self, idx: usize) {
        let Some(project) = &self.project else { return };
        let Some(stream) = project.streams.get(idx) else { return };
        let input = project.inputs[stream.source.input].path.clone();
        let index = stream.source.index;
        if let Some(view) = &mut self.cues
            && view.is_for(&input, index)
            && !matches!(view.load, CueLoad::Failed(_))
        {
            view.open = true;
            return;
        }
        let load = match self.cue_cache.get(&(input.clone(), index)) {
            Some(cues) => CueLoad::Ready(Arc::clone(cues)),
            None => CueLoad::Loading(subs::read_cues(&self.ffmpeg, &input, index)),
        };
        self.cues = Some(CueView::new(input, index, load));
    }

    /// Whether the cue card is open on `streams[idx]` (the inspector's View cues
    /// button then reads Hide cues).
    pub(crate) fn cues_show(&self, idx: usize) -> bool {
        let (Some(project), Some(view)) = (&self.project, &self.cues) else { return false };
        view.open && view.row(project) == Some(idx)
    }

    /// The row the open cue card shows, if any.
    pub(crate) fn cue_row(&self) -> Option<usize> {
        let (Some(project), Some(view)) = (&self.project, &self.cues) else { return None };
        view.open.then(|| view.row(project)).flatten()
    }

    /// Close the cue card, keeping its loaded cues for a quick reopen.
    pub(crate) fn close_cues(&mut self) {
        if let Some(view) = &mut self.cues {
            view.open = false;
        }
    }

    /// Re-read the cue card's stream after a failed load.
    fn retry_cues(&mut self) {
        let Some(view) = &self.cues else { return };
        let rx = subs::read_cues(&self.ffmpeg, &view.input, view.stream_index);
        if let Some(view) = &mut self.cues {
            view.load = CueLoad::Loading(rx);
        }
    }
}

/// Per-frame upkeep, whether or not the card is shown: take a finished load,
/// and drop the view once its stream is no longer in the project (another file
/// loaded, or the inserted track deleted).
pub(super) fn poll(ctx: &egui::Context, app: &mut InterlaceApp) {
    let Some(project) = &app.project else {
        app.cues = None;
        return;
    };
    let Some(view) = &mut app.cues else { return };
    if view.row(project).is_none() {
        app.cues = None;
        return;
    }
    if view.poll(&mut app.cue_cache) {
        ctx.request_repaint();
    }
}

/// The START field's flash strength now, `1.0` right after a cue click fading
/// to `0.0`, so the eye follows the value from the cue list down to the field.
/// Keeps repainting while it's still fading.
pub(super) fn start_flash(ctx: &egui::Context, app: &InterlaceApp) -> f32 {
    let Some(at) = app.start_flash else { return 0.0 };
    let t = at.elapsed().as_secs_f32();
    if t >= FLASH_SECS {
        return 0.0;
    }
    ctx.request_repaint();
    // Hold full strength for the first half, then fade out.
    (1.0 - (t / FLASH_SECS - 0.5).max(0.0) * 2.0).clamp(0.0, 1.0)
}

/// Whether the cue card should be stacked under the inspector this frame.
pub(super) fn is_open(app: &InterlaceApp) -> bool {
    app.cues.as_ref().is_some_and(|v| v.open)
}

/// What a click in the card asked for, applied after drawing.
enum CueAction {
    /// Set the preview start from cue `i`.
    Pick(usize),
    /// Same, then start the preview.
    Play(usize),
}

/// The cue card, filling `height`: header, OFFSET, filter, hint, then the list
/// (or the loading / failed / no-match state).
pub(super) fn panel(ui: &mut Ui, app: &mut InterlaceApp, height: f32) {
    let hint = app.start_flash.is_none(); // until the first cue click this session
    let can_play = app.preview_blocker().is_none();
    let Some(project) = &mut app.project else { return };
    let Some(view) = &mut app.cues else { return };
    let Some(row) = view.row(project) else { return };
    let mut close = false;
    let mut retry = false;
    let mut action: Option<CueAction> = None;

    widgets::card_fill(ui, height, Margin::symmetric(12, 10), |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        header(ui, project, row, &mut close);

        let offset = &mut project.streams[row].offset_secs;
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(vec2(52.0, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_size(vec2(52.0, 22.0));
                widgets::section_label(ui, "OFFSET");
            });
            widgets::offset_stepper(ui, offset, true);
            ui.add_space(4.0);
            line(ui, RichText::new("times include it").size(12.0).color(theme::WEAK));
        });
        let offset = *offset;

        match &view.load {
            CueLoad::Loading(_) => loading(ui),
            CueLoad::Failed(e) => retry = failed(ui, e),
            CueLoad::Ready(cues) => {
                let cues = Arc::clone(cues);
                let shown = filter_row(ui, &cues, &mut view.filter);
                if hint {
                    line(ui, RichText::new("Click a line to start the preview 2 s before it.").size(12.0).color(theme::WEAK));
                }
                if shown.is_empty() && !cues.is_empty() {
                    if no_match(ui, &view.filter) {
                        view.filter.clear();
                    }
                } else {
                    action = cue_list(ui, &cues, &shown, offset, view.selected, can_play);
                }
            }
        }
    });

    if close {
        view.open = false;
    }
    if retry {
        app.retry_cues();
        return;
    }
    // A clicked cue becomes the preview start, with a short lead-in so the line
    // isn't already on screen when the clip begins.
    let (CueAction::Pick(i) | CueAction::Play(i)) = match &action {
        Some(a) => a,
        None => return,
    };
    let i = *i;
    let Some(view) = &mut app.cues else { return };
    let CueLoad::Ready(cues) = &view.load else { return };
    let offset = app.project.as_ref().map_or(0.0, |p| p.streams[row].offset_secs);
    view.selected = Some(i);
    app.preview_start = subs::format_time((cues[i].start_secs + offset - CUE_LEAD_IN).max(0.0));
    app.start_flash = Some(std::time::Instant::now());
    if matches!(action, Some(CueAction::Play(_))) {
        app.start_preview();
    }
}

/// `CUES`, the subtitle badge with the stream id, the track's name, and ×.
fn header(ui: &mut Ui, project: &Project, row: usize, close: &mut bool) {
    let s = &project.streams[row];
    let name = s
        .meta
        .title
        .clone()
        .or_else(|| super::file_name_lossy(&project.inputs[s.source.input].path).filter(|_| s.source.input != 0))
        .unwrap_or_else(|| s.source.codec.clone());
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            *close = widgets::close_x(ui, 20.0, "Close cues");
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                widgets::section_label(ui, "CUES");
                widgets::pill(ui, &stream_id(project, row), theme::KIND_SUBTITLE, "");
                line(ui, RichText::new(name).font(theme::medium_font(13.0)));
            });
        });
    });
}

/// The filter field (magnifying glass, placeholder, clear button, accent
/// outline while filtering) and the cue count. Returns the indices of the cues
/// that match.
fn filter_row(ui: &mut Ui, cues: &[Cue], filter: &mut String) -> Vec<usize> {
    let needle = fold(filter.trim());
    let shown: Vec<usize> = (0..cues.len())
        .filter(|&i| needle.is_empty() || fold(&cues[i].text).contains(&needle))
        .collect();
    let count = if needle.is_empty() {
        format!("{} cues", cues.len())
    } else {
        format!("{} of {}", shown.len(), cues.len())
    };
    ui.horizontal(|ui| {
        let count_galley = ui
            .painter()
            .layout_no_wrap(count.clone(), egui::FontId::monospace(12.0), theme::WEAK);
        let field_width = ui.available_width() - count_galley.size().x - 8.0;
        let active = !filter.is_empty();
        let stroke = if active { theme::ACCENT } else { theme::STROKE };
        egui::Frame::NONE
            .fill(theme::FIELD)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 0))
            .show(ui, |ui| {
                ui.set_width(field_width - 18.0);
                ui.set_height(24.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(icon::MAGNIFYING_GLASS).size(14.0).color(theme::WEAK));
                let clear_width = if active { 22.0 } else { 0.0 };
                ui.add(
                    egui::TextEdit::singleline(filter)
                        .frame(egui::Frame::NONE)
                        .hint_text("Filter lines")
                        .desired_width(ui.available_width() - clear_width)
                        .margin(Margin::ZERO),
                );
                if active {
                    let resp = ui
                        .add(egui::Button::new(RichText::new(icon::X_CIRCLE).size(14.0).color(theme::WEAK)).frame(false))
                        .on_hover_text("Clear filter");
                    if resp.clicked() {
                        filter.clear();
                    }
                }
            });
        ui.label(RichText::new(count).font(egui::FontId::monospace(12.0)).color(theme::WEAK));
    });
    shown
}

/// Lowercase `s` with Latin accents stripped, so `xicara` finds `xícara`.
/// Covers the precomposed letters subtitles actually use; anything else passes
/// through unchanged.
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
            'ç' | 'ć' | 'č' => 'c',
            'ñ' | 'ń' | 'ň' => 'n',
            'ý' | 'ÿ' => 'y',
            'ś' | 'š' | 'ş' => 's',
            'ź' | 'ż' | 'ž' => 'z',
            'ł' => 'l',
            'ř' => 'r',
            'ť' => 't',
            'ď' => 'd',
            'ğ' => 'g',
            other => other,
        })
        .collect()
}

/// The filter matched nothing: say so, with a button to clear it. Returns
/// whether the button was clicked.
fn no_match(ui: &mut Ui, filter: &str) -> bool {
    ui.add_space(4.0);
    line(ui, RichText::new(format!("No line contains \u{201C}{}\u{201D}.", filter.trim())).color(theme::WEAK));
    ui.spacing_mut().button_padding = vec2(8.0, 2.0);
    ui.add(widgets::button(RichText::new(widgets::with_icon(icon::X, "Clear filter")).size(12.0)))
        .clicked()
}

/// The loading state: a spinning notch and what's happening.
fn loading(ui: &mut Ui) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
        let angle = ui.input(|i| i.time) as f32 * std::f32::consts::TAU;
        let galley = ui
            .painter()
            .layout_no_wrap(icon::CIRCLE_NOTCH.to_string(), egui::FontId::proportional(16.0), theme::ACCENT);
        // TextShape rotates about its top-left corner, so offset the corner by
        // the rotated half-size to spin the glyph about its center.
        let half = galley.size() / 2.0;
        let rot = egui::emath::Rot2::from_angle(angle);
        let pos = rect.center() - rot * half;
        ui.painter()
            .add(egui::epaint::TextShape::new(pos, galley, theme::ACCENT).with_angle(angle));
        ui.label("Reading subtitles…");
    });
    widgets::helper(ui, "ffmpeg reads the whole file once, usually 1–10 s. Reopening this track is instant.");
    ui.ctx().request_repaint();
}

/// The failed state: the reason in an error box, and Retry. Returns whether
/// Retry was clicked.
fn failed(ui: &mut Ui, error: &str) -> bool {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        widgets::status_line(ui, icon::X_CIRCLE, "Couldn't read the cues", theme::ERROR);
    });
    egui::Frame::NONE
        .fill(theme::FIELD)
        .stroke(Stroke::new(1.0, theme::ERROR_STROKE))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut job = egui::text::LayoutJob::single_section(
                error.to_string(),
                egui::TextFormat::simple(egui::FontId::monospace(12.0), theme::ERROR_TEXT),
            );
            job.wrap = egui::text::TextWrapping { max_width: ui.available_width(), break_anywhere: true, ..Default::default() };
            ui.add(egui::Label::new(job));
        });
    ui.add(widgets::button(widgets::with_icon(icon::ARROW_CLOCKWISE, "Retry"))).clicked()
}

/// The virtualized cue rows under a divider, filling the rest of the card.
fn cue_list(
    ui: &mut Ui,
    cues: &[Cue],
    shown: &[usize],
    offset: f64,
    selected: Option<usize>,
    can_play: bool,
) -> Option<CueAction> {
    let (rule, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rule, 0.0, theme::SECTION_DIVIDER);
    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("cue_rows")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, shown.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for &i in &shown[range] {
                if let Some(a) = cue_row(ui, &cues[i], i, offset, selected == Some(i), can_play) {
                    action = Some(a);
                }
            }
        });
    action
}

/// One cue: its start time (offset applied) and its text on one line, line
/// breaks shown as ` / `. Hovering shows a ▶ button and, after a moment, the
/// full text. Returns what a click asked for.
fn cue_row(ui: &mut Ui, cue: &Cue, i: usize, offset: f64, selected: bool, can_play: bool) -> Option<CueAction> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::hover());
    let resp = ui.interact(rect, egui::Id::new("cue_row").with(i), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp.clicked().then_some(CueAction::Pick(i));
    }
    // By pointer position, not `hovered()`: the ▶ button on top takes the
    // hover, and the row must not lose its hover state (and the button) to it.
    let hovered = ui.rect_contains_pointer(rect);
    let fill = if selected {
        theme::SELECTED
    } else if hovered {
        theme::CARD
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(4), fill);

    let painter = ui.painter();
    let time_color = if selected { theme::TEXT } else { theme::WEAK };
    let time = painter.layout_no_wrap(subs::format_time(cue.start_secs + offset), egui::FontId::monospace(12.5), time_color);
    let time_pos = pos2(rect.left() + 6.0, rect.center().y - time.size().y / 2.0);
    let text_left = time_pos.x + time.size().x + 10.0;
    painter.galley(time_pos, time, time_color);

    // Leave room for the ▶ button while it shows.
    let play_rect = egui::Rect::from_center_size(pos2(rect.right() - 4.0 - 11.0, rect.center().y), vec2(22.0, 18.0));
    let text_right = if hovered { play_rect.left() - 6.0 } else { rect.right() - 6.0 };
    let mut job = egui::text::LayoutJob::single_section(
        cue.text.replace('\n', " / "),
        egui::TextFormat::simple(egui::FontId::proportional(13.0), theme::TEXT),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((text_right - text_left).max(0.0));
    let text = painter.layout_job(job);
    painter.galley(pos2(text_left, rect.center().y - text.size().y / 2.0), text, theme::TEXT);

    let mut action = None;
    if hovered {
        let play = ui.interact(play_rect, egui::Id::new("cue_play").with(i), egui::Sense::click());
        let stroke = if play.hovered() { theme::ACCENT } else { theme::BUTTON_STROKE };
        ui.painter()
            .rect(play_rect, CornerRadius::same(4), theme::BUTTON, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        ui.painter().text(
            play_rect.center(),
            egui::Align2::CENTER_CENTER,
            icon::PLAY,
            egui::FontId::proportional(11.0),
            if can_play { theme::TEXT } else { theme::DIM },
        );
        if play.clicked() {
            action = Some(if can_play { CueAction::Play(i) } else { CueAction::Pick(i) });
        }
    }
    if action.is_none() && resp.clicked() {
        action = Some(CueAction::Pick(i));
    }
    resp.on_hover_ui(|ui| {
        ui.set_max_width(260.0);
        ui.add(egui::Label::new(cue.text.as_str()).wrap());
        ui.label(RichText::new("Click: set preview start · ▶ preview now").size(11.5).color(theme::WEAK));
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_ignores_case_and_accents() {
        assert_eq!(fold("Xícara ÇÃO"), "xicara cao");
        assert!(fold("Passagens, por favor.").contains(&fold("PASSAGENS")));
        assert_eq!(fold("日本語"), "日本語");
    }
}
