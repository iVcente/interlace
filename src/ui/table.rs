//! The stream table card, the centerpiece. One row per output stream in output
//! order: a drag handle, the type badge, what the stream is, its flags, and
//! pills for how it differs from the source. The validation hints sit under the
//! card, outside it.
//!
//! Columns are computed per frame from the card width: fixed columns keep their
//! size and the flexible ones share what's left, so every row lines up with the
//! header and long text truncates instead of pushing a column sideways. Narrow
//! windows show one STREAM summary column; wide ones (≥ 1400 px) split it into
//! CODEC · LANG · TITLE · SOURCE.
//!
//! Rows are drag-to-reorder: only the handle starts a drag, so the row body
//! stays clickable for selection, and a 2 px accent line shows where the row
//! will land. Row clicks are multi-select: Ctrl toggles a row, Shift selects a
//! range (see `selection.rs`); Ctrl+A / Esc / Delete select all, clear, and
//! remove the selection. Editing happens in the inspector; this table only
//! *shows* state.

use super::widgets::{self, badge, pill};
use super::{InterlaceApp, pick_media_file, stream_id, theme};
use crate::model::{Encode, Input, OutStream, Project};
use crate::validate::{self, Severity};
use egui::{Color32, FontId, Rect, RichText, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

/// Row height.
const ROW_HEIGHT: f32 = 34.0;
/// Header height: 7 px padding around 11 px caps.
const HEADER_HEIGHT: f32 = 28.0;
/// Horizontal padding inside the card, and the gap between columns.
const PAD: f32 = 10.0;
// Minimum height of the empty-state drop zone (it grows to fill the column).
const EMPTY_HEIGHT: f32 = 220.0;

/// What a column holds.
#[derive(Clone, Copy, PartialEq)]
enum Col {
    Handle,
    Type,
    /// Narrow layout: codec · lang · "title" · from file · offset, in one cell.
    Stream,
    Codec,
    Lang,
    Title,
    Source,
    Flags,
    Changes,
}

/// A column's width rule: fixed pixels, or a share of the leftover width.
#[derive(Clone, Copy)]
enum Width {
    Exact(f32),
    Fr(f32),
}

/// The column set for the window width.
fn columns(wide: bool) -> &'static [(Col, &'static str, Width)] {
    use Width::{Exact, Fr};
    if wide {
        &[
            (Col::Handle, "", Exact(16.0)),
            (Col::Type, "TYPE", Exact(62.0)),
            (Col::Codec, "CODEC", Exact(100.0)),
            (Col::Lang, "LANG", Exact(40.0)),
            (Col::Title, "TITLE", Fr(1.3)),
            (Col::Source, "SOURCE", Fr(1.0)),
            (Col::Flags, "FLAGS", Exact(60.0)),
            (Col::Changes, "CHANGES", Exact(200.0)),
        ]
    } else {
        &[
            (Col::Handle, "", Exact(16.0)),
            (Col::Type, "TYPE", Exact(62.0)),
            (Col::Stream, "STREAM", Fr(1.0)),
            (Col::Flags, "FLAGS", Exact(54.0)),
            (Col::Changes, "CHANGES", Exact(180.0)),
        ]
    }
}

/// Each column's horizontal span within a row of `width`, starting at `left`.
fn layout_columns(cols: &[(Col, &str, Width)], left: f32, width: f32) -> Vec<(Col, f32, f32)> {
    let inner = width - 2.0 * PAD;
    let gaps = PAD * (cols.len() - 1) as f32;
    let fixed: f32 = cols.iter().map(|(_, _, w)| if let Width::Exact(px) = w { *px } else { 0.0 }).sum();
    let frs: f32 = cols.iter().map(|(_, _, w)| if let Width::Fr(f) = w { *f } else { 0.0 }).sum();
    let leftover = (inner - fixed - gaps).max(40.0);
    let mut x = left + PAD;
    cols.iter()
        .map(|&(col, _, w)| {
            let px = match w {
                Width::Exact(px) => px,
                Width::Fr(f) => leftover * f / frs,
            };
            let span = (col, x, px);
            x += px + PAD;
            span
        })
        .collect()
}

pub(super) fn show(ui: &mut Ui, app: &mut InterlaceApp, wide: bool) {
    shortcuts(ui, app);

    // Lay the hints out first, so the card knows how much height they need and
    // the table can scroll inside the rest.
    let hints = hint_galleys(ui, app);
    let hints_height: f32 = hints.iter().map(|(_, _, g)| g.size().y + 4.0).sum();
    let max_card = (ui.available_height() - hints_height - if hints.is_empty() { 0.0 } else { 8.0 }).max(HEADER_HEIGHT + ROW_HEIGHT * 2.0);

    let cols = columns(wide);
    let mut click: Option<usize> = None;
    let mut reorder: Option<(usize, usize)> = None;
    let mut add_file = false;

    widgets::card_frame(egui::Margin::ZERO).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        header_row(ui, cols);

        let Some(project) = &app.project else {
            add_file = empty_state(ui);
            return;
        };

        let body_max = max_card - HEADER_HEIGHT - 2.0;
        egui::ScrollArea::vertical()
            .id_salt("stream_rows")
            .max_height(body_max)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let mut rects: Vec<Rect> = Vec::with_capacity(project.streams.len());
                for i in 0..project.streams.len() {
                    let last = i + 1 == project.streams.len();
                    let rect = stream_row(ui, cols, project, i, app.selection.contains(i), last, &mut click);
                    rects.push(rect);
                }
                if rects.is_empty() {
                    return;
                }
                let list = rects[0].union(*rects.last().unwrap());
                let resp = ui.interact(list, ui.id().with("row_drop"), egui::Sense::hover());
                let pointer = ui.ctx().pointer_interact_pos();
                let target = |p: egui::Pos2| rects.iter().filter(|r| p.y > r.center().y).count();
                if let (Some(p), Some(_)) = (pointer, resp.dnd_hover_payload::<usize>()) {
                    insertion_line(ui, &rects, target(p));
                }
                if let (Some(p), Some(from)) = (pointer, resp.dnd_release_payload::<usize>()) {
                    reorder = Some((*from, target(p)));
                }
            });
    });

    if !hints.is_empty() {
        ui.add_space(8.0);
        for (glyph, color, galley) in hints {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(glyph).size(13.5).color(color));
                ui.add(egui::Label::new(galley));
            });
            ui.add_space(4.0);
        }
    }

    // Apply mutations after the immutable borrow of `app.project` ends.
    if let Some(i) = click {
        let modifiers = ui.input(|input| input.modifiers);
        let len = app.project.as_ref().map_or(0, |p| p.streams.len());
        app.selection.click(i, modifiers, len);
    }
    if let Some((from, to)) = reorder {
        app.reorder(from, to);
    }
    if add_file && let Some(path) = pick_media_file() {
        app.load_file(path);
    }
}

/// The validation hints as wrapped galleys with their icon and color: errors
/// first, then warnings, each naming its stream by id. Hidden while the command
/// is hand-edited, since they describe the table, not the edited text.
fn hint_galleys(ui: &Ui, app: &InterlaceApp) -> Vec<(&'static str, Color32, std::sync::Arc<egui::Galley>)> {
    let Some(project) = &app.project else { return Vec::new() };
    if app.command_edit.is_some() {
        return Vec::new();
    }
    let wrap = (ui.available_width() - 20.0).max(100.0);
    validate::validate(project)
        .into_iter()
        .map(|issue| {
            let (glyph, color) = match issue.severity {
                Severity::Error => (icon::X_CIRCLE, theme::ERROR),
                Severity::Warning => (icon::WARNING, theme::WARNING),
            };
            let mut text = match issue.stream.filter(|&i| i < project.streams.len()) {
                Some(i) => format!("{}: {}", stream_id(project, i), issue.message),
                None => issue.message,
            };
            if let Some(s) = issue.suggestion {
                text.push(' ');
                text.push_str(&s);
            }
            let galley = ui.fonts_mut(|f| f.layout(text, FontId::proportional(12.5), color, wrap));
            (glyph, color, galley)
        })
        .collect()
}

/// Table-wide keyboard shortcuts: Ctrl+A selects every stream, Esc clears the
/// selection, Delete removes it. Skipped while a text field has focus, so typing
/// a title or editing the command bar (where these keys mean text editing) is
/// never hijacked — and while a combo popup or the overwrite modal is up, where
/// Esc means "close this", not "deselect".
fn shortcuts(ui: &Ui, app: &mut InterlaceApp) {
    let Some(len) = app.project.as_ref().map(|p| p.streams.len()) else { return };
    let ctx = ui.ctx();
    if ctx.egui_wants_keyboard_input()
        || egui::Popup::is_any_open(ctx)
        || app.pending_run.is_some()
        || app.run_state.is_main_running()
    {
        return;
    }
    let (all, clear, remove) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Delete),
        )
    });
    if all {
        app.selection.select_all(len);
    }
    if clear {
        app.selection.clear();
    }
    if remove {
        app.remove_selected();
    }
}

/// The header row: caps labels over each column and a bottom border.
fn header_row(ui: &mut Ui, cols: &[(Col, &str, Width)]) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    for ((_, title, _), (_, x, _)) in cols.iter().zip(layout_columns(cols, rect.left(), rect.width())) {
        if title.is_empty() {
            continue;
        }
        let galley = painter.layout_job(widgets::caps(title, theme::WEAK));
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, theme::WEAK);
    }
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme::STROKE));
}

/// The no-file state: a dashed drop zone inviting a drop or a pick. Returns
/// whether `Add file…` was clicked.
fn empty_state(ui: &mut Ui) -> bool {
    let margin = 14.0;
    let width = ui.available_width();
    // Fill the card down to the bottom of the column, so the whole main area
    // reads as the drop target.
    let height = ui.available_height().max(EMPTY_HEIGHT + 2.0 * margin);
    let (outer, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
    let zone = outer.shrink(margin);
    widgets::dashed_rect(ui.painter(), zone, 8.0, Stroke::new(1.0, theme::BUTTON_STROKE));

    let mut clicked = false;
    let content_height = 32.0 + 8.0 + 20.0 + 8.0 + 26.0 + 8.0 + 16.0;
    let top = zone.center().y - content_height / 2.0;
    let mut child = ui.new_child(
        egui::UiBuilder::new().id_salt("empty_state")
            .max_rect(Rect::from_min_max(pos2(zone.left(), top), pos2(zone.right(), zone.bottom())))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.y = 8.0;
    child.label(RichText::new(icon::FILE_VIDEO).size(32.0).color(theme::WEAK));
    child.label(RichText::new("Drop a video file here").size(15.0).color(theme::TEXT));
    clicked |= child.add(widgets::accent_button(&widgets::with_icon(icon::PLUS, "Add file…"))).clicked();
    child.label(RichText::new("mkv · mp4 · mov · webm · avi").size(12.0).color(theme::WEAK));
    clicked
}

/// Draw one stream row; returns its rect. Sets `click` when the row body (not
/// the drag handle) is clicked.
fn stream_row(
    ui: &mut Ui,
    cols: &[(Col, &str, Width)],
    project: &Project,
    i: usize,
    selected: bool,
    last: bool,
    click: &mut Option<usize>,
) -> Rect {
    let s = &project.streams[i];
    let input = &project.inputs[s.source.input];
    // An explicit per-row id (not an auto id), so rows keep their identity when
    // the layout around the table changes between passes.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::hover());
    let resp = ui.interact(rect, egui::Id::new("stream_row").with(i), egui::Sense::click());
    if resp.clicked() {
        *click = Some(i);
    }
    if !ui.is_rect_visible(rect) {
        return rect;
    }

    let fill = if selected {
        theme::SELECTED
    } else if s.removed {
        theme::REMOVED
    } else if resp.hovered() {
        theme::CARD
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    if !last {
        ui.painter()
            .hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme::ROW_DIVIDER));
    }

    let text_color = if s.removed { theme::WEAK } else { theme::TEXT };
    let text = |t: String| {
        let rich = RichText::new(t).color(text_color);
        if s.removed { rich.strikethrough() } else { rich }
    };

    for (c, (col, x, w)) in layout_columns(cols, rect.left(), rect.width()).into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(w, ROW_HEIGHT));
        let mut cui = ui.new_child(
            egui::UiBuilder::new().id_salt(("row_cell", i, c))
                .max_rect(cell)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        cui.set_clip_rect(cell.intersect(ui.clip_rect()));
        cui.spacing_mut().item_spacing.x = 4.0;
        match col {
            Col::Handle => {
                // Only the handle starts a drag; the payload is the row index.
                cui.dnd_drag_source(egui::Id::new("stream_dnd").with(i), i, drag_handle);
            }
            Col::Type => {
                badge(&mut cui, s.source.kind);
            }
            Col::Stream => {
                widgets::line(&mut cui, text(summary(s, input)));
            }
            Col::Codec => {
                widgets::line(&mut cui, text(codec_text(s)));
            }
            Col::Lang => {
                widgets::line(&mut cui, text(s.meta.language.clone().unwrap_or_else(|| "—".into())));
            }
            Col::Title => {
                widgets::line(&mut cui, text(s.meta.title.clone().unwrap_or_else(|| "—".into())));
            }
            Col::Source => {
                let src = match s.source.input {
                    0 => "0".to_string(),
                    n => format!(
                        "{n} · {}",
                        super::file_name_lossy(&input.path).unwrap_or_default()
                    ),
                };
                widgets::line(&mut cui, RichText::new(src).color(theme::WEAK));
            }
            Col::Flags => {
                widgets::line(&mut cui, RichText::new(flags(s)).size(12.0).color(theme::WEAK));
            }
            Col::Changes => {
                for c in changes(s) {
                    pill(&mut cui, c.label, c.color, &c.hover);
                }
            }
        }
    }
    rect
}

/// The drag handle: Phosphor's six-dot grip, brighter on hover.
fn drag_handle(ui: &mut Ui) {
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0, ROW_HEIGHT), egui::Sense::hover());
    let color = if resp.hovered() { theme::WEAK } else { theme::DIM };
    ui.painter()
        .text(rect.center(), egui::Align2::CENTER_CENTER, icon::DOTS_SIX_VERTICAL, FontId::proportional(14.0), color);
}

/// The 2 px accent line at the row boundary a drag would drop on.
fn insertion_line(ui: &Ui, rects: &[Rect], to: usize) {
    let y = if to == 0 { rects[0].top() } else { rects[to.min(rects.len()) - 1].bottom() };
    let x = rects[0].x_range();
    ui.painter().hline(x, y, Stroke::new(2.0, theme::ACCENT));
}

/// One change indicator for the CHANGES column.
struct Change {
    label: &'static str,
    color: Color32,
    hover: String,
}

/// The pills describing how `s` differs from its probed original. A removed
/// stream shows only the red `remove` pill; otherwise any of `embed` / `convert`
/// / `retag` / `flags` / `offset` that apply (empty when the stream is untouched).
fn changes(s: &OutStream) -> Vec<Change> {
    if s.removed {
        return vec![Change {
            label: "remove",
            color: theme::CHANGE_REMOVE,
            hover: "Will be dropped from the output".into(),
        }];
    }

    let mut out = Vec::new();
    // An embedded external track: synthetic *and* drawn from an added input (a
    // converted-copy sibling is also synthetic but stays on the primary input).
    if s.added && s.source.input != 0 {
        out.push(Change {
            label: "embed",
            color: theme::CHANGE_EMBED,
            hover: "Inserted from an added file".into(),
        });
    }
    if s.converted() {
        out.push(Change {
            label: "convert",
            color: theme::CHANGE_CONVERT,
            hover: format!("Re-encode {}", codec_text(s)),
        });
    }
    if s.tags_changed() {
        out.push(Change {
            label: "retag",
            color: theme::CHANGE_RETAG,
            hover: tag_diff(s),
        });
    }
    if s.flags_changed() {
        out.push(Change {
            label: "flags",
            color: theme::CHANGE_FLAGS,
            hover: format!(
                "default {} » {}, forced {} » {}",
                on_off(s.orig_meta.default),
                on_off(s.meta.default),
                on_off(s.orig_meta.forced),
                on_off(s.meta.forced),
            ),
        });
    }
    // A meaningful sync offset on this stream (same ≥1ms gate as `to_args`).
    if s.has_offset() {
        out.push(Change {
            label: "offset",
            color: theme::CHANGE_OFFSET,
            hover: format!("Timestamps shifted {}", super::format_offset(s.offset_secs)),
        });
    }
    out
}

/// `on`/`off` for the flags pill's hover text.
fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// A human summary of the language/title edits for the `retag` pill's hover.
fn tag_diff(s: &OutStream) -> String {
    let mut parts = Vec::new();
    if s.meta.language != s.orig_meta.language {
        parts.push(format!(
            "language: {} » {}",
            opt(&s.orig_meta.language),
            opt(&s.meta.language)
        ));
    }
    if s.meta.title != s.orig_meta.title {
        parts.push(format!(
            "title: {} » {}",
            opt(&s.orig_meta.title),
            opt(&s.meta.title)
        ));
    }
    parts.join("\n")
}

/// A tag value in curly quotes, or `—` when unset.
fn opt(v: &Option<String>) -> String {
    match v {
        Some(s) => format!("\u{201C}{s}\u{201D}"),
        None => "—".into(),
    }
}

/// The codec cell: the source codec, or `source » target` when re-encoding.
fn codec_text(s: &OutStream) -> String {
    match &s.encode {
        Encode::Copy => s.source.codec.clone(),
        Encode::Audio { codec, .. } => format!("{} » {}", s.source.codec, codec),
    }
}

/// The narrow layout's one-cell summary: `aac » opus · por · "Portuguese"`, plus
/// ` · from subs.srt` for an inserted track and ` · −0.5 s` for any offset.
fn summary(s: &OutStream, input: &Input) -> String {
    let mut parts: Vec<String> = vec![codec_text(s)];
    if let Some(lang) = &s.meta.language {
        parts.push(lang.clone());
    }
    if let Some(title) = &s.meta.title {
        parts.push(format!("\u{201C}{title}\u{201D}"));
    }
    // Inserted tracks come from a non-primary input: name the file it came from.
    if s.source.input != 0 {
        let name = super::file_name_lossy(&input.path)
            .unwrap_or_else(|| format!("input {}", s.source.input));
        parts.push(format!("from {name}"));
    }
    if s.has_offset() {
        parts.push(short_offset(s.offset_secs));
    }
    parts.join(" · ")
}

/// The offset with trailing zeros trimmed, for the compact summary: `+0.15 s`,
/// `−1.5 s`. Rounded to the millisecond the command actually uses.
fn short_offset(secs: f64) -> String {
    let ms = (secs * 1000.0).round() / 1000.0;
    let sign = if ms < 0.0 { '−' } else { '+' };
    format!("{sign}{} s", ms.abs())
}

/// The FLAGS cell: `default`, `forced`, both, or nothing.
fn flags(s: &OutStream) -> String {
    let mut f = Vec::new();
    if s.meta.default {
        f.push("default");
    }
    if s.meta.forced {
        f.push("forced");
    }
    f.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Meta, Source};
    use std::path::PathBuf;

    fn out(input: usize, added: bool, meta: Meta) -> OutStream {
        let mut s = OutStream::new(
            Source { input, index: 0, kind: Kind::Subtitle, codec: "subrip".into(), bitrate_kbps: None },
            meta,
            Encode::Copy,
        );
        s.added = added;
        s
    }

    fn input(name: &str) -> Input {
        Input::new(PathBuf::from(name))
    }

    /// `s` with its sync offset set to `secs`.
    fn shifted(mut s: OutStream, secs: f64) -> OutStream {
        s.offset_secs = secs;
        s
    }

    fn labels(cs: &[Change]) -> Vec<&str> {
        cs.iter().map(|c| c.label).collect()
    }

    #[test]
    fn embedded_summary_names_file_and_offset() {
        let s = shifted(out(1, true, Meta { language: Some("eng".into()), ..Default::default() }), 0.15);
        let text = summary(&s, &input("/media/subs.srt"));
        assert_eq!(text, "subrip · eng · from subs.srt · +0.15 s");
    }

    #[test]
    fn embedded_summary_without_offset_omits_it() {
        let s = out(1, true, Meta::default());
        let text = summary(&s, &input("subs.srt"));
        assert_eq!(text, "subrip · from subs.srt");
    }

    /// A track inside the primary file shows its offset too, with a real minus.
    #[test]
    fn primary_track_summary_shows_offset() {
        let s = shifted(out(0, false, Meta::default()), -1.5);
        assert_eq!(summary(&s, &input("in.mkv")), "subrip · −1.5 s");
        assert_eq!(labels(&changes(&s)), ["offset"]);
    }

    #[test]
    fn embed_and_offset_badges_on_embedded_track() {
        let s = shifted(out(1, true, Meta::default()), -0.2);
        assert_eq!(labels(&changes(&s)), ["embed", "offset"]);
    }

    #[test]
    fn no_embed_badge_for_primary_input_or_converted_sibling() {
        // A synthetic convert-sibling stays on input 0 → not an embed.
        let sibling = out(0, true, Meta::default());
        assert!(!labels(&changes(&sibling)).contains(&"embed"));
        // A plain probed primary stream → no badges at all.
        let primary = out(0, false, Meta::default());
        assert!(changes(&primary).is_empty());
    }

    /// Fixed columns keep their width and the flexible ones split the rest, so
    /// the narrow STREAM column at a 628 px card matches the mockup's 256 px.
    #[test]
    fn columns_share_leftover_width() {
        let spans = layout_columns(columns(false), 0.0, 628.0);
        let stream = spans.iter().find(|(c, _, _)| *c == Col::Stream).unwrap();
        assert!((stream.2 - 256.0).abs() < 0.5, "stream width {}", stream.2);
        let last = spans.last().unwrap();
        assert!((last.1 + last.2 - (628.0 - PAD)).abs() < 0.5, "columns end at the right padding");
    }
}
