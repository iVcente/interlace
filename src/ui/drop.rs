//! Drag and drop: a full-window overlay with **one action per drop**.
//!
//! The app holds one main video plus any number of single-track files, so every
//! drop maps to exactly one plan: open the video, replace it, insert tracks, or
//! replace and then insert. Files are classified by extension while they hover
//! (ffprobe confirms after the drop); a second video or a non-media file is
//! listed as skipped rather than guessed at. The overlay names the plan before
//! the release, so nothing a drop does is a surprise.
//!
//! Replacing a main file that has edits asks first (`pending_drop`). Every
//! executed drop is one undo step.

use super::widgets::{self, line};
use super::{InterlaceApp, stream_id, theme};
use crate::model::{Kind, Project};
use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;
use std::path::{Path, PathBuf};

/// Extensions opened as the main file.
const VIDEO_EXTENSIONS: &[&str] = &["mkv", "mp4", "m4v", "mov", "webm", "avi", "ts", "m2ts"];
/// Audio track extensions (inserted as tracks). Also the `+ Audio` picker's
/// filter, so what the picker offers and what a drop inserts can't drift apart.
pub(crate) const AUDIO_EXTENSIONS: &[&str] = &["mka", "ac3", "eac3", "dts", "flac", "aac", "m4a", "opus", "mp3", "wav", "ogg", "thd"];
/// Subtitle track extensions (inserted as tracks), and the `+ Subtitle`
/// picker's filter.
pub(crate) const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "sup", "vtt", "sub"];

/// What a hovered file is, by extension.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum FileKind {
    Video,
    Audio,
    Subtitle,
    /// Not media we handle; skipped.
    Other,
}

/// Classify `path` by its extension, case-insensitively.
pub(crate) fn classify(path: &Path) -> FileKind {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let is = |list: &[&str]| list.contains(&ext.as_str());
    if is(VIDEO_EXTENSIONS) {
        FileKind::Video
    } else if is(AUDIO_EXTENSIONS) {
        FileKind::Audio
    } else if is(SUBTITLE_EXTENSIONS) {
        FileKind::Subtitle
    } else {
        FileKind::Other
    }
}

/// What releasing the files will do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DropAction {
    /// Load the video as the main file (nothing is open).
    Open,
    /// Close the open file and load the video instead.
    Replace,
    /// Insert this many tracks into the open file.
    Insert(usize),
    /// Open (or replace with, when `replace`) the video, then insert `n` tracks.
    OpenThenInsert { replace: bool, n: usize },
    /// Nothing happens, for this reason.
    Refused(&'static str),
}

/// A classified drop: the files in the order shown, and the one action.
#[derive(Clone, Debug)]
pub(crate) struct DropPlan {
    /// Every file with its kind and, when it's left out, why.
    pub(crate) files: Vec<(PathBuf, FileKind, Option<&'static str>)>,
    /// The video that becomes the main file, if any.
    pub(crate) video: Option<PathBuf>,
    /// The track files to insert, in drop order.
    pub(crate) tracks: Vec<PathBuf>,
    pub(crate) action: DropAction,
}

impl DropPlan {
    /// Plan a drop of `paths` given whether a file is open and a job is running.
    pub(crate) fn new(paths: Vec<PathBuf>, project_open: bool, running: bool) -> Self {
        let mut video = None;
        let mut tracks = Vec::new();
        let mut files = Vec::new();
        for path in paths {
            let kind = classify(&path);
            let skipped = match kind {
                FileKind::Video if video.is_some() => Some("skipped · only one video"),
                FileKind::Video => {
                    video = Some(path.clone());
                    None
                }
                FileKind::Audio | FileKind::Subtitle => {
                    tracks.push(path.clone());
                    None
                }
                FileKind::Other => Some("skipped · not media"),
            };
            files.push((path, kind, skipped));
        }
        let action = if running {
            DropAction::Refused("Wait for the run to finish")
        } else {
            match (&video, tracks.len()) {
                (None, 0) => DropAction::Refused("Nothing here to open or insert"),
                (None, _) if !project_open => DropAction::Refused("Open a video first"),
                (None, n) => DropAction::Insert(n),
                (Some(_), 0) if project_open => DropAction::Replace,
                (Some(_), 0) => DropAction::Open,
                (Some(_), n) => DropAction::OpenThenInsert { replace: project_open, n },
            }
        };
        Self { files, video, tracks, action }
    }

    /// Whether this plan closes the open file.
    fn replaces(&self) -> bool {
        matches!(self.action, DropAction::Replace | DropAction::OpenThenInsert { replace: true, .. })
    }
}

/// How many edits the open project carries: changed streams (removed,
/// converted, retagged, flags, offset, added copies) and a reorder (counted
/// once). The container title isn't counted: it's seeded from the source, so a
/// set title doesn't mean an edit. Track files are counted by the caller.
pub(crate) fn edit_count(project: &Project) -> usize {
    let streams = project
        .streams
        .iter()
        .filter(|s| s.source.input == 0)
        .filter(|s| s.removed || s.converted() || s.tags_changed() || s.flags_changed() || s.has_offset() || s.added)
        .count();
    let mut order = project.streams.iter().filter(|s| s.source.input == 0 && !s.added).map(|s| s.source.index);
    let reordered = order.clone().zip(order.by_ref().skip(1)).any(|(a, b)| a > b);
    streams + usize::from(reordered)
}

impl InterlaceApp {
    /// Act on files released over the window: run the plan, or hold it for
    /// confirmation when it would discard edits.
    pub(crate) fn open_dropped(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let plan = DropPlan::new(paths, self.project.is_some(), self.run_state.is_running());
        if matches!(plan.action, DropAction::Refused(_)) {
            return;
        }
        let edits = self.project.as_ref().is_some_and(|p| edit_count(p) > 0 || p.inputs.len() > 1);
        if plan.replaces() && edits {
            self.pending_drop = Some(plan);
        } else {
            self.run_drop(plan);
        }
    }

    /// Execute `plan` as one undo step: load the video (if any), then insert
    /// each track after the last stream of its kind, selecting the first new
    /// row. Failures are collected rather than stopping at the first.
    pub(crate) fn run_drop(&mut self, plan: DropPlan) {
        let before = self.snapshot();
        let mut errors = Vec::new();
        if let Some(video) = &plan.video {
            self.load_file(video.clone());
            if let Some(e) = self.error.take() {
                // The old project is still there; inserting into it would
                // surprise more than help.
                self.error = Some(e);
                return;
            }
        }
        let mut added = Vec::new();
        for track in &plan.tracks {
            self.embed_file(track.clone());
            match self.error.take() {
                Some(e) => errors.push(e),
                None => added.push(track.clone()),
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

        let name = |p: &Path| super::file_name_lossy(p).unwrap_or_else(|| p.display().to_string());
        let text = match (&plan.video, added.as_slice(), first) {
            (Some(v), [], _) if plan.replaces() => format!("Replaced the main file with {}", name(v)),
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

    /// The confirmation for a drop that replaces a file with edits.
    pub(super) fn replace_modal(&mut self, ctx: &egui::Context) {
        let Some(plan) = &self.pending_drop else { return };
        let message = self
            .project
            .as_ref()
            .map(|p| replace_description(p, plan))
            .unwrap_or_default();
        let (mut go, mut cancel) = (false, false);
        egui::Window::new(RichText::new(widgets::with_icon(icon::ARROW_SQUARE_IN, "Replace the main file?")).font(theme::medium_font(14.0)))
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add(egui::Label::new(RichText::new(message).color(theme::WEAK)).wrap());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    go = ui.add(widgets::accent_button("Replace")).clicked();
                    cancel = ui.add(widgets::button("Cancel")).clicked();
                });
            });
        if go {
            if let Some(plan) = self.pending_drop.take() {
                self.run_drop(plan);
            }
        } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.pending_drop = None;
        }
    }
}

/// `s` for a count other than one.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// "X will be closed. Your 6 edits and 1 track file will be discarded."
fn replace_description(project: &Project, plan: &DropPlan) -> String {
    let current = super::file_name_lossy(&project.inputs[0].path).unwrap_or_default();
    let edits = edit_count(project);
    let track_files = project.inputs.len() - 1;
    let mut lost = Vec::new();
    if edits > 0 {
        lost.push(format!("Your {edits} edit{}", plural(edits)));
    }
    if track_files > 0 {
        let lead = if lost.is_empty() { "Your " } else { "" };
        lost.push(format!("{lead}{track_files} track file{}", plural(track_files)));
    }
    let mut text = format!("{current} will be closed.");
    if !lost.is_empty() {
        text.push_str(&format!(" {} will be discarded.", lost.join(" and ")));
    }
    if let (Some(v), n) = (&plan.video, plan.tracks.len())
        && n > 0
    {
        let name = super::file_name_lossy(v).unwrap_or_default();
        text = format!("Opens {name} as the main file, then adds {n} track{}. {text}", plural(n));
    }
    text
}

/// The overlay while files hover over the window.
pub(super) fn overlay(ctx: &egui::Context, app: &InterlaceApp) {
    let hovered: Vec<PathBuf> = ctx.input(|i| i.raw.hovered_files.iter().filter_map(|f| f.path.clone()).collect());
    let count = ctx.input(|i| i.raw.hovered_files.len());
    if count == 0 {
        return;
    }
    let plan = DropPlan::new(hovered, app.project.is_some(), app.run_state.is_running());
    // Cover the main column (where the table or the empty drop zone is), not
    // the whole window, so the drop reads as landing in the table.
    let area = if app.drop_area.is_positive() { app.drop_area } else { ctx.content_rect() };
    let pad = 16.0;
    egui::Area::new(egui::Id::new("drop_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(area.min)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::tint(theme::BG, 0xF7))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(pad as i8))
                .show(ui, |ui| {
                    let inner = area.size() - vec2(2.0 * pad, 2.0 * pad);
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    ui.spacing_mut().item_spacing.y = 12.0;
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let n = count.max(plan.files.len());
                        widgets::section_label(ui, &format!("DROPPING {n} FILE{}", if n == 1 { "" } else { "S" }));
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                            for (path, kind, skipped) in &plan.files {
                                file_chip(ui, path, *kind, *skipped);
                            }
                        });
                    });
                    let footer_height = 18.0;
                    let target_height = (ui.available_height() - footer_height - 12.0).max(120.0);
                    target(ui, app.project.as_ref(), &plan, target_height);
                    line(ui, RichText::new(footer_text(&plan)).size(12.5).color(theme::WEAK));
                });
        });
}

/// One hovered file: kind icon, name, and kind word (or why it's skipped).
fn file_chip(ui: &mut Ui, path: &Path, kind: FileKind, skipped: Option<&str>) {
    let (glyph, color, word) = match kind {
        FileKind::Video => (icon::FILE_VIDEO, theme::KIND_VIDEO, "video"),
        FileKind::Audio => (icon::WAVEFORM, theme::KIND_AUDIO, "audio"),
        FileKind::Subtitle => (icon::SUBTITLES, theme::KIND_SUBTITLE, "subtitle"),
        FileKind::Other => (icon::PROHIBIT, theme::ERROR, "not media"),
    };
    let name = super::file_name_lossy(path).unwrap_or_else(|| path.display().to_string());
    let stroke = if skipped.is_some() { theme::ERROR } else { theme::STROKE };
    let (word, word_color) = match skipped {
        Some(why) => (why, theme::ERROR),
        None => (word, color),
    };
    // Measure the chip up front: a wrapped row can only break before a widget
    // whose size it knows, and a frame's size is known only after drawing it.
    let measure = |text: &str, size: f32| {
        ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(size), theme::TEXT).size().x
    };
    let name_width = measure(&name, 13.0).min(300.0);
    let chip_width = 16.0 + measure(glyph, 14.0) + 6.0 + name_width + 6.0 + measure(word, 12.0) + 2.0;
    // In a wrapped row `available_width` is the whole row, so measure from the cursor.
    let left_on_row = ui.max_rect().right() - ui.cursor().min.x;
    if left_on_row < chip_width && ui.cursor().min.x > ui.max_rect().left() + 1.0 {
        ui.end_row();
    }
    egui::Frame::NONE
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_max_width(420.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let glyph_color = if skipped.is_some() { theme::ERROR } else { color };
                ui.label(RichText::new(glyph).size(14.0).color(glyph_color));
                let text = RichText::new(name);
                let text = if skipped.is_some() { text.strikethrough().color(theme::WEAK) } else { text };
                let width = name_width;
                ui.allocate_ui_with_layout(vec2(width, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_max_width(width);
                    line(ui, text);
                });
                ui.label(RichText::new(word).size(12.0).color(word_color));
            });
        });
}

/// The dashed target filling the middle: icon, title, description and key
/// hint, centered in it.
fn target(ui: &mut Ui, project: Option<&Project>, plan: &DropPlan, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
    let (glyph, color, title, description, hint) = describe(project, plan);
    ui.painter().rect_filled(rect, CornerRadius::same(10), theme::tint(color, 0x1A));
    widgets::dashed_rect(ui.painter(), rect.shrink(1.0), 10.0, Stroke::new(2.0, color));

    let inner = rect.shrink(22.0);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("drop_target")
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    // Center the block both ways. The description wraps, so its lines are
    // centered too, and it's measured first to size the block.
    let mut job = egui::text::LayoutJob::simple(description, egui::FontId::proportional(13.0), theme::WEAK, inner.width());
    job.halign = egui::Align::Center;
    let desc = child.fonts_mut(|f| f.layout_job(job));
    let block = 30.0 + 10.0 + 22.0 + 8.0 + desc.size().y + 10.0 + 16.0;
    child.add_space(((inner.height() - block) / 2.0).max(0.0));
    child.spacing_mut().item_spacing.y = 8.0;
    child.label(RichText::new(glyph).size(30.0).color(color));
    child.label(RichText::new(title).font(theme::medium_font(17.0)).color(theme::TEXT));
    child.add(egui::Label::new(desc));
    if !hint.is_empty() {
        child.label(RichText::new(hint).font(egui::FontId::monospace(11.5)).color(theme::WEAK));
    }
}

/// Icon, border color, title, description and key hint for `plan`.
fn describe(project: Option<&Project>, plan: &DropPlan) -> (&'static str, Color32, String, String, &'static str) {
    let name = |p: &Path| super::file_name_lossy(p).unwrap_or_default();
    let has_edits = project.is_some_and(|p| edit_count(p) > 0 || p.inputs.len() > 1);
    let then = if has_edits { "release · then confirm" } else { "release to replace" };
    match &plan.action {
        DropAction::Open => (
            icon::FOLDER_OPEN,
            theme::ACCENT,
            "Open".into(),
            format!("{} becomes the main file.", plan.video.as_deref().map(name).unwrap_or_default()),
            "release to open",
        ),
        DropAction::Replace => {
            let text = match project {
                Some(p) if has_edits => format!("{} You'll be asked to confirm.", replace_description(p, plan)),
                Some(p) => format!("{} will be closed. It has no edits.", name(&p.inputs[0].path)),
                None => String::new(),
            };
            (icon::ARROW_SQUARE_IN, theme::WARNING, "Replace the main file".into(), text, then)
        }
        DropAction::Insert(n) => {
            let title = format!("Insert {n} track{}", plural(*n));
            let text = match (project, plan.tracks.as_slice()) {
                (Some(p), [one]) => insert_one_description(p, one),
                _ => "Each goes after the last stream of its kind, in the order dropped. Drag rows afterwards to move them.".into(),
            };
            (icon::ROWS_PLUS_BOTTOM, theme::ACCENT, title, text, "release to insert")
        }
        DropAction::OpenThenInsert { replace, n } => {
            let video = plan.video.as_deref().map(name).unwrap_or_default();
            let verb = if *replace { "Replace the video" } else { "Open the video" };
            let title = format!("{verb}, then insert {n} track{}", plural(*n));
            let mut text = format!("Opens {video} as the main file, then adds the {n} track{}.", plural(*n));
            if *replace && has_edits {
                text.push_str(" Current edits are discarded after you confirm.");
            }
            let color = if *replace { theme::WARNING } else { theme::ACCENT };
            let hint = if *replace { then } else { "release to open" };
            (icon::ARROW_SQUARE_IN, color, title, text, hint)
        }
        DropAction::Refused(why) => (
            icon::PROHIBIT,
            theme::DIM,
            (*why).to_string(),
            "Releasing here does nothing.".into(),
            "",
        ),
    }
}

/// "S1 · subtitle, placed after S0. Drag the row afterwards to move it."
fn insert_one_description(project: &Project, track: &Path) -> String {
    let kind = match classify(track) {
        FileKind::Audio => Kind::Audio,
        _ => Kind::Subtitle,
    };
    let same: Vec<usize> = (0..project.streams.len()).filter(|&i| project.streams[i].source.kind == kind).collect();
    let letter = kind.spec().to_ascii_uppercase();
    let word = widgets::kind_text(kind);
    let placed = match same.last() {
        Some(&last) => format!("placed after {}", stream_id(project, last)),
        None => "placed at the end".into(),
    };
    format!("{letter}{} · {word}, {placed}. Drag the row afterwards to move it.", same.len())
}

/// The footer line under the target.
fn footer_text(plan: &DropPlan) -> &'static str {
    match plan.action {
        DropAction::Insert(_) => "Release anywhere to insert · Esc cancels",
        DropAction::Open | DropAction::OpenThenInsert { replace: false, .. } => "Release anywhere to open · Esc cancels",
        DropAction::Replace | DropAction::OpenThenInsert { replace: true, .. } => {
            "Release anywhere to replace · you'll confirm next · Esc cancels"
        }
        DropAction::Refused(_) => "Esc cancels",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(files: &[&str], open: bool) -> DropPlan {
        DropPlan::new(files.iter().map(PathBuf::from).collect(), open, false)
    }

    #[test]
    fn classify_by_extension() {
        assert_eq!(classify(Path::new("a.MKV")), FileKind::Video);
        assert_eq!(classify(Path::new("a.ac3")), FileKind::Audio);
        assert_eq!(classify(Path::new("a.srt")), FileKind::Subtitle);
        assert_eq!(classify(Path::new("notes.txt")), FileKind::Other);
        assert_eq!(classify(Path::new("no_ext")), FileKind::Other);
    }

    #[test]
    fn one_action_per_drop() {
        assert_eq!(plan(&["m.mkv"], false).action, DropAction::Open);
        assert_eq!(plan(&["m.mkv"], true).action, DropAction::Replace);
        assert_eq!(plan(&["a.srt", "b.ac3"], true).action, DropAction::Insert(2));
        assert_eq!(plan(&["a.srt"], false).action, DropAction::Refused("Open a video first"));
        assert_eq!(
            plan(&["a.srt", "m.mkv", "b.ac3"], true).action,
            DropAction::OpenThenInsert { replace: true, n: 2 }
        );
        assert!(matches!(plan(&["notes.txt"], true).action, DropAction::Refused(_)));
        let running = DropPlan::new(vec![PathBuf::from("m.mkv")], false, true);
        assert_eq!(running.action, DropAction::Refused("Wait for the run to finish"));
    }

    /// A second video and non-media files are listed as skipped, not used.
    #[test]
    fn extra_videos_and_other_files_are_skipped() {
        let p = plan(&["a.mkv", "b.mp4", "x.txt", "s.srt"], true);
        assert_eq!(p.video, Some(PathBuf::from("a.mkv")));
        assert_eq!(p.tracks, [PathBuf::from("s.srt")]);
        let skipped: Vec<bool> = p.files.iter().map(|(_, _, s)| s.is_some()).collect();
        assert_eq!(skipped, [false, true, true, false]);
    }
}
