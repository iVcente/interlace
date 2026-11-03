//! The two cards at the bottom, each in a three-column grid (button · label ·
//! content). They're separate because the preview's settings only shape the temp
//! clip, and side by side they read as settings of the export.
//!
//! The run card shows the OUTPUT path with a browse button; running, a progress
//! bar with percent, elapsed time and speed (Run turns into Cancel); finished,
//! one line saying what was written or why it failed, with Show in folder or
//! View log. Messages are always one truncated line, so nothing overlaps the bar.
//!
//! The preview card, above it because previewing comes first, has a START time,
//! a 10/30/60 s length, and a dropdown each for the clip's audio and subtitle
//! track. Its own progress or failure takes the dropdowns' place, as does the
//! reason when Preview is unavailable. An invalid START keeps the dropdowns,
//! since half-typed times shouldn't make the row jump; the field turns red and
//! the button's hover says why. Run and Preview share the one ffmpeg slot, so
//! each disables the other.

use super::widgets::{self, line};
use super::{InterlaceApp, JobKind, Outcome, RunState, format_offset, pick_output_file, short_name, theme};
use crate::model::{Kind, Project};
use crate::preview::{self, TrackPick};
use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;

/// The grid's column widths and gaps (see the module doc).
const BUTTON_COL: f32 = 104.0;
const LABEL_COL: f32 = 56.0;
const COL_GAP: f32 = 12.0;
const ROW_HEIGHT: f32 = 28.0;

/// What the user asked for this frame, applied after drawing.
#[derive(Default)]
struct Actions {
    run: bool,
    cancel: bool,
    preview: bool,
    browse: bool,
    show_folder: Option<std::path::PathBuf>,
    view_log: bool,
    dismiss: bool,
}

pub(super) fn show(ui: &mut Ui, app: &mut InterlaceApp) {
    let mut act = Actions::default();
    ui.spacing_mut().item_spacing.y = theme::GUTTER;
    if app.project.is_some() {
        card_row(ui, |ui| preview_row(ui, app, &mut act));
    }
    card_row(ui, |ui| run_row(ui, app, &mut act));

    if act.run {
        app.on_run_clicked();
    }
    if act.cancel {
        app.cancel_job();
    }
    if act.preview {
        app.start_preview();
    }
    if act.view_log {
        app.show_log = true;
    }
    if act.dismiss {
        app.run_state = RunState::Idle;
        app.show_log = false;
    }
    if let Some(path) = act.show_folder
        && let Err(e) = crate::run::show_in_folder(&path)
    {
        app.error = Some(e);
    }
    if act.browse
        && let Some(project) = &mut app.project
        && let Some(path) = pick_output_file(&project.output)
    {
        project.output = path;
    }
}

/// A card holding one grid row.
fn card_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    widgets::card(ui, Margin::symmetric(12, 8), |ui| {
        ui.spacing_mut().item_spacing = vec2(COL_GAP, 6.0);
        ui.horizontal(|ui| {
            ui.set_height(ROW_HEIGHT);
            add(ui);
        });
    });
}

/// The row's trailing buttons, from the right: dismiss, then an optional `extra`
/// as `(icon, label)`. Returns whether `extra` was clicked.
fn trailing_buttons(ui: &mut Ui, act: &mut Actions, extra: Option<(&str, &str)>) -> bool {
    let mut clicked = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        act.dismiss = widgets::close_x(ui, 20.0, "Dismiss");
        if let Some((glyph, label)) = extra {
            clicked = ui.add(widgets::button(widgets::with_icon(glyph, label))).clicked();
        }
    });
    clicked
}

/// Run (or Cancel), then the output path, progress, or the last run's outcome. A
/// preview's progress and outcome stay in the preview card.
fn run_row(ui: &mut Ui, app: &mut InterlaceApp, act: &mut Actions) {
    // The button.
    let main_running = app.run_state.is_main_running();
    if main_running {
        let cancel = widgets::button(widgets::with_icon(icon::STOP, "Cancel"));
        act.cancel = ui.add_sized(vec2(BUTTON_COL, ROW_HEIGHT), cancel).clicked();
    } else {
        let blocker = app.run_blocker();
        let btn = if blocker.is_none() {
            widgets::accent_button(&widgets::with_icon(icon::PLAY, "Run"))
        } else {
            egui::Button::new(
                RichText::new(widgets::with_icon(icon::PLAY, "Run")).font(theme::medium_font(13.0)).color(theme::WEAK),
            )
            .fill(theme::CARD)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(6))
        };
        // Drawn enabled-looking either way (the styling carries the state), so
        // the hover reason still shows.
        let resp = ui.add_sized(vec2(BUTTON_COL, ROW_HEIGHT), btn.sense(if blocker.is_none() {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        }));
        match blocker {
            Some(why) => {
                resp.on_hover_text(why);
            }
            None => act.run = resp.on_hover_text("Write the output file").clicked(),
        }
    }

    // The rest of the row.
    match &app.run_state {
        RunState::Running { job, fraction, speed, .. } if !job.is_preview() => {
            let elapsed = job.started.elapsed().as_secs();
            let speed = speed.map(|s| format!(" · {s:.1}×")).unwrap_or_default();
            let text = format!("{:.0}% · {:02}:{:02}{speed}", fraction * 100.0, elapsed / 60, elapsed % 60);
            progress(ui, *fraction, &text);
        }
        RunState::Done { job, outcome } if !job.is_preview() => match outcome {
            Outcome::Wrote { bytes, secs } => {
                let output = match &job.kind {
                    JobKind::Run { output } => output.clone(),
                    JobKind::Preview { .. } => None,
                };
                let what = output.as_ref().map_or("the output".to_string(), |p| p.display().to_string());
                let size = bytes.map(|b| format!(" · {}", human_size(b))).unwrap_or_default();
                let text = format!("Wrote {what}{size} in {}", human_secs(*secs));
                message(ui, icon::CHECK_CIRCLE, &text, theme::SUCCESS, |ui| {
                    let extra = output.is_some().then_some((icon::FOLDER_OPEN, "Show in folder"));
                    if trailing_buttons(ui, act, extra) {
                        act.show_folder = output.clone();
                    }
                });
            }
            Outcome::Cancelled => {
                message(ui, icon::INFO, "Run cancelled. The output file may be incomplete.", theme::WEAK, |ui| {
                    trailing_buttons(ui, act, None);
                });
            }
            Outcome::Failed { message: text, .. } => failure(ui, act, text),
            Outcome::PreviewOpened => {} // never a run's outcome
        },
        _ => match app.missing_binary().filter(|(name, ..)| *name == "ffmpeg") {
            Some((name, tried, err)) => {
                let text = format!("{name} not found — Tried {tried} — {err}. Set the path in Settings, or install ffmpeg and add it to PATH.");
                message(ui, icon::X_CIRCLE, &text, theme::ERROR, |_| {});
            }
            None => output_row(ui, app, act),
        },
    }
}

/// The idle row: OUTPUT, the path field, and the browse button.
fn output_row(ui: &mut Ui, app: &mut InterlaceApp, act: &mut Actions) {
    ui.add_sized(vec2(LABEL_COL, ROW_HEIGHT), egui::Label::new(widgets::caps("OUTPUT", theme::WEAK)));
    let field_width = ui.available_width() - 28.0 - COL_GAP;
    ui.allocate_ui_with_layout(vec2(field_width, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        output_field(ui, app, field_width);
    });
    let browse = ui.add_enabled_ui(app.project.is_some(), |ui| {
        widgets::icon_button(ui, icon::FOLDER_OPEN, vec2(28.0, 24.0), false, "Choose where to write the output")
    });
    act.browse = browse.inner.clicked();
}

/// The output path. At rest it's drawn as a field that keeps the *end* of the
/// path visible (the file name is what matters), with the full path on hover;
/// a click turns it into a text edit until it loses focus. Both forms center the
/// text vertically, so it stays put across the swap.
fn output_field(ui: &mut Ui, app: &mut InterlaceApp, width: f32) {
    let id = egui::Id::new("output_path_edit");
    let Some(project) = &mut app.project else {
        let (rect, _) = ui.allocate_exact_size(vec2(width, 26.0), egui::Sense::hover());
        paint_field(ui, rect, false);
        ui.painter().text(
            rect.left_center() + vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "Choose an output file…",
            egui::FontId::proportional(13.0),
            theme::WEAK,
        );
        return;
    };
    if app.editing_output {
        let mut text = project.output.display().to_string();
        let resp = ui.add_sized(
            vec2(width, 26.0),
            egui::TextEdit::singleline(&mut text)
                .id(id)
                .margin(Margin::symmetric(8, 4))
                .vertical_align(egui::Align::Center),
        );
        if resp.changed() {
            project.output = text.into();
        }
        if resp.lost_focus() {
            app.editing_output = false;
        }
        return;
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 26.0), egui::Sense::click());
    paint_field(ui, rect, resp.hovered());
    let full = project.output.display().to_string();
    let font = egui::FontId::proportional(13.0);
    let (shown, cut) = elide_start(ui, &full, &font, rect.width() - 16.0);
    ui.painter()
        .text(rect.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, shown, font, theme::TEXT);
    let resp = resp.on_hover_cursor(egui::CursorIcon::Text);
    let resp = if cut { resp.on_hover_text(&full) } else { resp };
    if resp.clicked() {
        app.editing_output = true;
        ui.memory_mut(|m| m.request_focus(id));
        // Redraw before this frame ends: the edit that now has focus isn't in it
        // yet, and AccessKit panics on a focused node missing from its tree.
        ui.ctx().request_discard("the output path became a text edit");
    }
}

/// A text-field look for a custom-drawn field.
fn paint_field(ui: &Ui, rect: egui::Rect, hovered: bool) {
    let stroke = if hovered { theme::BUTTON_STROKE } else { theme::STROKE };
    ui.painter()
        .rect(rect, CornerRadius::same(6), theme::FIELD, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
}

/// `text` cut from the *start* with a leading `…` so it fits `width`. Returns
/// the text to draw and whether anything was cut.
fn elide_start(ui: &Ui, text: &str, font: &egui::FontId, width: f32) -> (String, bool) {
    let fits = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x) <= width;
    if fits(text) {
        return (text.to_string(), false);
    }
    // Binary-search the shortest cut (in chars) whose remainder fits.
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (0, starts.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if fits(&format!("…{}", &text[starts[mid]..])) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let tail = starts.get(lo).map_or("", |&i| &text[i..]);
    (format!("…{tail}"), true)
}

/// The flex-width progress bar (6 px, never with text inside) plus its mono
/// readout on the right.
fn progress(ui: &mut Ui, fraction: f32, text: &str) {
    let font = egui::FontId::monospace(12.5);
    let readout = ui.painter().layout_no_wrap(text.to_string(), font, theme::WEAK);
    let bar_width = (ui.available_width() - readout.size().x - COL_GAP).max(40.0);
    let (rect, _) = ui.allocate_exact_size(vec2(bar_width, ROW_HEIGHT), egui::Sense::hover());
    let track = egui::Rect::from_center_size(rect.center(), vec2(rect.width(), 6.0));
    ui.painter().rect_filled(track, CornerRadius::same(3), theme::FIELD);
    let mut fill = track;
    fill.set_width(track.width() * fraction.clamp(0.0, 1.0));
    ui.painter().rect_filled(fill, CornerRadius::same(3), theme::ACCENT);
    let (rect, _) = ui.allocate_exact_size(readout.size(), egui::Sense::hover());
    ui.painter().galley(rect.min, readout, theme::WEAK);
}

/// A one-line status message spanning the label and content columns, with the
/// row's trailing buttons laid out by `trailing` from the right.
fn message(ui: &mut Ui, glyph: &str, text: &str, color: Color32, trailing: impl FnOnce(&mut Ui)) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        trailing(ui);
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            widgets::status_line(ui, glyph, text, color);
        });
    });
}

/// A failed job's one-line reason, with View log and dismiss.
fn failure(ui: &mut Ui, act: &mut Actions, text: &str) {
    message(ui, icon::X_CIRCLE, text, theme::ERROR, |ui| {
        act.view_log = trailing_buttons(ui, act, Some((icon::FILE_TEXT, "View log")));
    });
}

/// Preview (or Cancel), START, the length switch, then the track dropdowns, or
/// the clip's progress, failure, or why it can't start.
fn preview_row(ui: &mut Ui, app: &mut InterlaceApp, act: &mut Actions) {
    let preview_running = matches!(&app.run_state, RunState::Running { job, .. } if job.is_preview());
    let blocker = app.preview_blocker();
    if preview_running {
        let cancel = widgets::button(widgets::with_icon(icon::STOP, "Cancel"));
        act.cancel = ui.add_sized(vec2(BUTTON_COL, ROW_HEIGHT), cancel).clicked();
    } else {
        let btn = widgets::button(widgets::with_icon(icon::FILM_SLATE, "Preview"));
        let resp = ui.add_enabled_ui(blocker.is_none(), |ui| ui.add_sized(vec2(BUTTON_COL, ROW_HEIGHT), btn)).inner;
        let resp = match blocker {
            Some(why) => resp.on_disabled_hover_text(why),
            None => resp,
        };
        act.preview = resp
            .on_hover_text("Render this stretch of the output to a temp file and open it in your player")
            .clicked();
    }

    ui.add_sized(vec2(LABEL_COL, ROW_HEIGHT), egui::Label::new(widgets::caps("START", theme::WEAK)));
    let valid = preview::parse_timestamp(&app.preview_start).is_some();
    ui.scope(|ui| {
        if !valid {
            let v = &mut ui.visuals_mut().widgets;
            v.inactive.bg_stroke = Stroke::new(1.0, theme::ERROR);
            v.hovered.bg_stroke = Stroke::new(1.0, theme::ERROR);
        }
        let flash = super::cues::start_flash(ui.ctx(), app);
        if flash > 0.0 {
            let v = ui.visuals_mut();
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, theme::tint(theme::ACCENT, (255.0 * flash) as u8));
            v.text_edit_bg_color = Some(theme::FIELD.lerp_to_gamma(theme::ACCENT, 0.1 * flash));
        }
        ui.add_sized(
            vec2(118.0, 26.0),
            egui::TextEdit::singleline(&mut app.preview_start)
                .font(egui::FontId::monospace(12.5))
                .text_color(if valid { theme::TEXT } else { theme::ERROR })
                .margin(Margin::symmetric(8, 4))
                .horizontal_align(egui::Align::Center)
                .vertical_align(egui::Align::Center)
                .hint_text("00:12:30"),
        )
        .on_hover_text("Start time in the output: hh:mm:ss, mm:ss or seconds. Clicking a subtitle cue sets it.");
    });
    length_switch(ui, &mut app.preview_len);
    match &app.run_state {
        RunState::Running { job, fraction, .. } if job.is_preview() => {
            let len = match &job.kind {
                JobKind::Preview { len, .. } => *len,
                JobKind::Run { .. } => 0.0,
            };
            progress(ui, *fraction, &format!("{:.0}% · rendering {len} s clip", fraction * 100.0));
        }
        RunState::Done { job, outcome: Outcome::Failed { message: text, .. } } if job.is_preview() => failure(ui, act, text),
        _ => match blocker.filter(|_| valid) {
            Some(why) => blocked_line(ui, why),
            None => track_pickers(ui, app),
        },
    }
}

/// The `10 s | 30 s | 60 s` segmented control.
fn length_switch(ui: &mut Ui, len: &mut f64) {
    egui::Frame::NONE
        .fill(theme::FIELD)
        .stroke(Stroke::new(1.0, theme::STROKE))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Each segment is painted at a fixed 40×20: an `egui::Button` sizes
            // itself from its outline too, so the selected one (outlined) was
            // wider and the row shifted on every switch.
            for option in preview::LENGTHS {
                let on = *len == option;
                let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 20.0), egui::Sense::click());
                if resp.clicked() {
                    *len = option;
                }
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let (fill, stroke, color) = match (on, resp.hovered()) {
                    (true, _) => (theme::BUTTON, theme::BUTTON_STROKE, theme::TEXT),
                    (false, true) => (theme::CARD, Color32::TRANSPARENT, theme::TEXT),
                    (false, false) => (Color32::TRANSPARENT, Color32::TRANSPARENT, theme::WEAK),
                };
                ui.painter()
                    .rect(rect, CornerRadius::same(4), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{option} s"),
                    egui::FontId::proportional(12.0),
                    color,
                );
            }
        });
}

/// Why the preview can't start, in place of the dropdowns.
fn blocked_line(ui: &mut Ui, why: &str) {
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.label(RichText::new(format!("{} ", icon::INFO)).size(14.0).color(theme::WEAK));
    line(ui, RichText::new(why).size(12.5).color(theme::WEAK));
}

/// The AUDIO and SUBTITLE dropdowns, sharing the rest of the row evenly.
fn track_pickers(ui: &mut Ui, app: &mut InterlaceApp) {
    let (Some(project), Some(picks)) = (&app.project, &mut app.preview_picks) else { return };
    let labels = ui.painter().layout_no_wrap("AUDIOSUBTITLE".into(), egui::FontId::proportional(13.0), theme::WEAK).size().x;
    let width = ((ui.available_width() - labels - 3.0 * COL_GAP) / 2.0).clamp(90.0, 340.0);
    track_picker(ui, project, Kind::Audio, "AUDIO", &mut picks.audio, width);
    track_picker(ui, project, Kind::Subtitle, "SUBTITLE", &mut picks.subtitle, width);
}

/// One kind's dropdown: each live track, then None. Selecting a track in the
/// table sets it too (`InterlaceApp::update_preview_picks`).
fn track_picker(ui: &mut Ui, project: &Project, kind: Kind, label: &str, pick: &mut TrackPick, width: f32) {
    let name = |row: usize| {
        let s = &project.streams[row];
        let offset = if s.has_offset() { format!(" {}", format_offset(s.offset_secs)) } else { String::new() };
        format!("{}{offset}", short_name(project, row))
    };
    let rows: Vec<usize> = (0..project.streams.len())
        .filter(|&i| project.streams[i].source.kind == kind && !project.streams[i].removed)
        .collect();
    let (text, color) = match pick.row(project) {
        Some(row) => (name(row), theme::TEXT),
        None => ("None".to_string(), theme::WEAK),
    };

    ui.label(widgets::caps(label, theme::WEAK));
    let mut picked = None;
    let combo = super::inspector::field_combo(ui, |ui| {
        ui.add_enabled_ui(!rows.is_empty(), |ui| {
            egui::ComboBox::from_id_salt(("preview_track", label))
                .width(width)
                .truncate()
                .selected_text(RichText::new(text).size(12.5).color(color))
                .show_ui(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for &row in &rows {
                        if widgets::option_row(ui, &name(row), pick.row(project) == Some(row)).clicked() {
                            picked = Some(TrackPick::track(project, row));
                        }
                    }
                    if widgets::option_row(ui, "None", *pick == TrackPick::Off).clicked() {
                        picked = Some(TrackPick::Off);
                    }
                });
        })
    });
    combo.response.on_hover_text("The track the clip uses. Selecting one in the table sets it too. None leaves this kind out of the clip.");
    if let Some(new) = picked {
        *pick = new;
    }
}

/// The View log window: the failed job's stderr, scrollable, with Copy.
pub(super) fn log_window(ctx: &egui::Context, app: &mut InterlaceApp) {
    if !app.show_log {
        return;
    }
    let RunState::Done { outcome: Outcome::Failed { log, .. }, .. } = &app.run_state else {
        app.show_log = false;
        return;
    };
    let mut open = true;
    let mut copy = false;
    egui::Window::new(RichText::new("ffmpeg log").font(theme::medium_font(14.0)))
        .open(&mut open)
        .collapsible(false)
        .default_size(vec2(640.0, 360.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            copy = ui.add(widgets::button(widgets::with_icon(icon::COPY, "Copy"))).clicked();
            ui.add_space(6.0);
            egui::Frame::NONE
                .fill(theme::FIELD)
                .stroke(Stroke::new(1.0, theme::STROKE))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    egui::ScrollArea::both().max_height(320.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(log.as_str()).font(egui::FontId::monospace(12.0)).color(theme::CODE)).extend());
                    });
                });
        });
    if copy {
        ctx.copy_text(log.clone());
    }
    app.show_log = open;
}

/// A byte count as `1.41 GB` / `350 MB` / `12 KB`.
fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

/// A duration as `38 s` or `4 min 12 s`.
fn human_secs(secs: f64) -> String {
    let s = secs.round() as u64;
    if s < 60 { format!("{s} s") } else { format!("{} min {} s", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_and_secs_read_naturally() {
        assert_eq!(human_size(1_410_000_000), "1.41 GB");
        assert_eq!(human_size(350_000_000), "350 MB");
        assert_eq!(human_secs(38.2), "38 s");
        assert_eq!(human_secs(252.0), "4 min 12 s");
    }
}
