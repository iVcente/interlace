//! The run card at the bottom: two rows in a three-column grid (buttons ·
//! labels · content).
//!
//! Row 1 is the run itself. Idle, it shows the OUTPUT path with a browse
//! button; running, a progress bar with percent, elapsed time and speed (Run
//! turns into Cancel); finished, one line saying what was written or why it
//! failed, with Show in folder or View log. Messages are always one truncated
//! line, so nothing overlaps the bar.
//!
//! Row 2 is the preview clip: a START time, a 10/30/60 s length, and a line
//! naming the track under test (or why Preview is unavailable). Run and Preview
//! share the one ffmpeg slot, so each disables the other; a rendering preview
//! reports its progress in row 1 under a PREVIEW label.

use super::widgets::{self, line};
use super::{InterlaceApp, JobKind, Outcome, RunState, format_offset, pick_output_file, short_name, theme};
use crate::preview;
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
    widgets::card(ui, Margin::symmetric(12, 8), |ui| {
        ui.spacing_mut().item_spacing = vec2(COL_GAP, 6.0);
        ui.horizontal(|ui| {
            ui.set_height(ROW_HEIGHT);
            run_row(ui, app, &mut act);
        });
        if app.project.is_some() {
            ui.horizontal(|ui| {
                ui.set_height(ROW_HEIGHT);
                preview_row(ui, app, &mut act);
            });
        }
    });

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

/// Row 1: Run (or Cancel), then the output path, progress, or the last job's
/// outcome.
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
        RunState::Running { job, fraction, speed, .. } => {
            let elapsed = job.started.elapsed().as_secs();
            let text = match &job.kind {
                JobKind::Preview { len, .. } => {
                    ui.add_sized(vec2(LABEL_COL, ROW_HEIGHT), egui::Label::new(widgets::caps("PREVIEW", theme::WEAK)));
                    format!("{:.0}% · rendering {len} s clip", fraction * 100.0)
                }
                JobKind::Run { .. } => {
                    let speed = speed.map(|s| format!(" · {s:.1}×")).unwrap_or_default();
                    format!("{:.0}% · {:02}:{:02}{speed}", fraction * 100.0, elapsed / 60, elapsed % 60)
                }
            };
            progress(ui, *fraction, &text);
        }
        RunState::Done { job, outcome } => {
            let right = |ui: &mut Ui, act: &mut Actions, extra: Option<(&str, &str)>| -> bool {
                // Lays out the trailing buttons; returns whether the extra one was clicked.
                let mut clicked = false;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    act.dismiss = widgets::close_x(ui, 20.0, "Dismiss");
                    if let Some((glyph, label)) = extra {
                        clicked = ui.add(widgets::button(widgets::with_icon(glyph, label))).clicked();
                    }
                });
                clicked
            };
            match outcome {
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
                        if right(ui, act, extra) {
                            act.show_folder = output.clone();
                        }
                    });
                }
                Outcome::PreviewOpened => {
                    let (start, len) = match &job.kind {
                        JobKind::Preview { start, len, .. } => (*start, *len),
                        JobKind::Run { .. } => (0.0, 0.0),
                    };
                    let text = format!(
                        "Preview opened in your default player · {}, {len} s",
                        crate::subs::format_time(start)
                    );
                    message(ui, icon::CHECK_CIRCLE, &text, theme::SUCCESS, |ui| {
                        right(ui, act, None);
                    });
                }
                Outcome::Cancelled => {
                    let text = if job.is_preview() {
                        "Preview cancelled."
                    } else {
                        "Run cancelled. The output file may be incomplete."
                    };
                    message(ui, icon::INFO, text, theme::WEAK, |ui| {
                        right(ui, act, None);
                    });
                }
                Outcome::Failed { message: text, .. } => {
                    message(ui, icon::X_CIRCLE, text, theme::ERROR, |ui| {
                        act.view_log = right(ui, act, Some((icon::FILE_TEXT, "View log")));
                    });
                }
            }
        }
        RunState::Idle => match app.missing_binary().filter(|(name, ..)| *name == "ffmpeg") {
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
/// a click turns it into a text edit until it loses focus.
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
            egui::TextEdit::singleline(&mut text).id(id).margin(Margin::symmetric(8, 4)),
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

/// Row 2: Preview (or Cancel), START, the length switch, and what's tested.
fn preview_row(ui: &mut Ui, app: &mut InterlaceApp, act: &mut Actions) {
    let preview_running = matches!(&app.run_state, RunState::Running { job, .. } if job.is_preview());
    let blocker = app.preview_blocker();
    if preview_running {
        let cancel = widgets::button(widgets::with_icon(icon::STOP, "Cancel"));
        act.cancel = ui.add_sized(vec2(BUTTON_COL, ROW_HEIGHT), cancel).clicked();
    } else {
        let btn = widgets::button(widgets::with_icon(icon::FILM_SLATE, "Preview"));
        let resp = ui.add_enabled_ui(blocker.is_none(), |ui| ui.add_sized(vec2(BUTTON_COL, ROW_HEIGHT), btn)).inner;
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
    info_line(ui, app, blocker);
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

/// What the preview will test, or why it can't start.
fn info_line(ui: &mut Ui, app: &InterlaceApp, blocker: Option<&str>) {
    ui.spacing_mut().item_spacing.x = 0.0;
    if let Some(why) = blocker {
        let invalid_start = app.project.is_some()
            && !app.run_state.is_running()
            && app.command_edit.is_none()
            && preview::parse_timestamp(&app.preview_start).is_none();
        let (glyph, color) = if invalid_start { (icon::WARNING, theme::ERROR) } else { (icon::INFO, theme::WEAK) };
        ui.label(RichText::new(format!("{glyph} ")).size(14.0).color(color));
        line(ui, RichText::new(why).size(12.5).color(color));
        return;
    }
    let Some(project) = &app.project else { return };
    let mut job = egui::text::LayoutJob::default();
    let fmt = |color| egui::TextFormat::simple(egui::FontId::proportional(12.5), color);
    match preview::focus(project, app.preview_subject()) {
        Some(idx) => {
            let s = &project.streams[idx];
            job.append(&format!("{} ", icon::TARGET), 0.0, egui::TextFormat::simple(egui::FontId::proportional(14.0), theme::CHANGE_OFFSET));
            job.append("Testing ", 0.0, fmt(theme::WEAK));
            job.append(&short_name(project, idx), 0.0, fmt(theme::TEXT));
            if s.has_offset() {
                job.append(
                    &format_offset(s.offset_secs),
                    6.0,
                    egui::TextFormat::simple(egui::FontId::monospace(12.0), theme::CHANGE_OFFSET),
                );
            }
            let others = if s.source.kind == crate::model::Kind::Audio { "other audio" } else { "other subtitles" };
            job.append(&format!(" · {others} left out"), 0.0, fmt(theme::WEAK));
        }
        None => {
            job.append(&format!("{} ", icon::INFO), 0.0, egui::TextFormat::simple(egui::FontId::proportional(14.0), theme::WEAK));
            job.append("Select an audio or subtitle track to test it on its own", 0.0, fmt(theme::WEAK));
        }
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width(ui.available_width());
    ui.add(egui::Label::new(job));
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
