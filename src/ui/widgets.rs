//! Small drawing helpers shared by every card: the card frame itself, section
//! labels, buttons in their four styles, kind badges, change pills, the custom
//! checkbox and the sync-offset stepper.
//!
//! They exist so each screen region reads as layout rather than styling, and so
//! one token change in `theme` reaches every place that uses it. Single-line
//! text in a constrained spot always goes through `Label::truncate` (egui then
//! shows the full text on hover), so no widget can widen its column.

use super::theme::{self, medium_font};
use crate::model::Kind;
use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Vec2, vec2};
use egui_phosphor::regular as icon;

/// The rounded card every panel draws itself into (fill `PANEL`, 1 px stroke).
pub(super) fn card_frame(margin: impl Into<Margin>) -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::STROKE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(margin)
}

/// A card spanning the available width, sized by its content.
pub(super) fn card<R>(ui: &mut egui::Ui, margin: impl Into<Margin>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card_frame(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A card filling exactly `height` (and the available width), for the cards
/// that pin content to their bottom edge.
pub(super) fn card_fill<R>(
    ui: &mut egui::Ui,
    height: f32,
    margin: impl Into<Margin>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let margin = margin.into();
    let width = ui.available_width();
    ui.allocate_ui(vec2(width, height), |ui| {
        card_frame(margin)
            .show(ui, |ui| {
                let inner = vec2(
                    width - margin.sum().x - 2.0,
                    (height - margin.sum().y - 2.0).max(0.0),
                );
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                add(ui)
            })
            .inner
    })
    .inner
}

/// An 11 px caps label (`SOURCES`, `TAGS`…) with a little letter spacing.
pub(super) fn section_label(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Label::new(caps(text, theme::WEAK)).selectable(false))
}

/// The caps-label text as a layout job, for callers that paint it themselves.
pub(super) fn caps(text: &str, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(11.0),
            color,
            extra_letter_spacing: 0.66,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job
}

/// Weak helper text at 12 px that wraps.
pub(super) fn helper(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(theme::WEAK)).wrap());
}

/// A single truncated line of `text` (the full text shows on hover when cut).
pub(super) fn line(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Label::new(text).truncate())
}

/// `icon` then `text`, with the 5 px gap the buttons use.
pub(super) fn with_icon(glyph: &str, text: &str) -> String {
    if text.is_empty() { glyph.to_string() } else { format!("{glyph} {text}") }
}

/// The normal button: `BUTTON` fill, `BUTTON_STROKE` outline.
pub(super) fn button(text: impl Into<egui::WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(theme::BUTTON)
        .stroke(Stroke::new(1.0, theme::BUTTON_STROKE))
        .corner_radius(CornerRadius::same(6))
}

/// The primary action button (Run, Add file…, Overwrite): accent fill.
pub(super) fn accent_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).font(medium_font(13.0)).color(theme::ON_ACCENT))
        .fill(theme::ACCENT)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(6))
}

/// The outlined destructive button (Remove): transparent with a red outline.
pub(super) fn danger_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(theme::REMOVE_TEXT))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, theme::REMOVE_STROKE))
        .corner_radius(CornerRadius::same(6))
}

/// A toggle-state button (Hide cues): `SELECTED` fill and an accent outline.
pub(super) fn active_button(text: impl Into<egui::WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(theme::SELECTED)
        .stroke(Stroke::new(1.0, theme::ACCENT))
        .corner_radius(CornerRadius::same(6))
}

/// A fixed-size icon-only button with a tooltip. `active` fills it with the
/// accent (the command-panel and settings toggles while open).
///
/// Painted rather than built from `egui::Button`: a `Button` sizes itself from
/// its content and outline, and `add_sized` only sets a minimum, so toggling
/// the outline off for the active state made it narrower and shifted its
/// neighbors. Here the footprint is exactly `size` in every state.
pub(super) fn icon_button(ui: &mut egui::Ui, glyph: &str, size: Vec2, active: bool, hover: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() && ui.is_enabled();
        let (fill, color, stroke) = match (active, hovered) {
            (true, _) => (theme::ACCENT, theme::ON_ACCENT, theme::ACCENT),
            (false, true) => (ui.visuals().widgets.hovered.weak_bg_fill, theme::TEXT, ui.visuals().widgets.hovered.bg_stroke.color),
            (false, false) => (theme::BUTTON, theme::TEXT, theme::BUTTON_STROKE),
        };
        let fade = |c: Color32| if ui.is_enabled() { c } else { c.gamma_multiply(0.4) };
        ui.painter()
            .rect(rect, CornerRadius::same(6), fade(fill), Stroke::new(1.0, fade(stroke)), egui::StrokeKind::Inside);
        ui.painter()
            .text(rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(15.0), fade(color));
    }
    resp.on_hover_text(hover)
}

/// A small borderless `x` that turns into a filled square on hover. Shared by
/// card headers, chips and toasts. Returns `true` on click.
pub(super) fn close_x(ui: &mut egui::Ui, size: f32, hover: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
    let resp = resp.on_hover_text(hover);
    if ui.is_rect_visible(rect) {
        let color = if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(4), theme::BUTTON);
            theme::TEXT
        } else {
            theme::WEAK
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon::X,
            egui::FontId::proportional(size * 0.62),
            color,
        );
    }
    resp.clicked()
}

/// The filled, rounded type badge (video/audio/subtitle/attach).
pub(super) fn badge(ui: &mut egui::Ui, kind: Kind) -> egui::Response {
    filled_tag(ui, kind_text(kind), kind_color(kind), Margin::symmetric(7, 2))
}

/// A change pill (`remove`, `convert`…) with its own hover text.
pub(super) fn pill(ui: &mut egui::Ui, text: &str, color: Color32, hover: &str) {
    let resp = filled_tag(ui, text, color, Margin::symmetric(6, 2));
    if !hover.is_empty() {
        resp.on_hover_text(hover);
    }
}

/// White 11 px text on a radius-4 `fill`.
/// Painted rather than built from a `Frame`, so it keeps its own height
/// whatever the row around it is (a frame stretches to a tall cell).
fn filled_tag(ui: &mut egui::Ui, text: &str, fill: Color32, margin: Margin) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), egui::FontId::proportional(11.0), Color32::WHITE);
    let size = galley.size() + margin.sum();
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        let pos = rect.min + vec2(margin.left as f32, margin.top as f32);
        ui.painter().galley(pos, galley, Color32::WHITE);
    }
    resp
}

/// The badge color for each stream kind.
pub(super) fn kind_color(kind: Kind) -> Color32 {
    match kind {
        Kind::Video => theme::KIND_VIDEO,
        Kind::Audio => theme::KIND_AUDIO,
        Kind::Subtitle => theme::KIND_SUBTITLE,
        Kind::Attachment | Kind::Data => theme::KIND_OTHER,
    }
}

/// The badge word for each stream kind.
pub(super) fn kind_text(kind: Kind) -> &'static str {
    match kind {
        Kind::Video => "video",
        Kind::Audio => "audio",
        Kind::Subtitle => "subtitle",
        Kind::Attachment => "attach",
        Kind::Data => "data",
    }
}

/// The 14 px checkbox from the mockup: a `FIELD` box that fills with the accent
/// and shows a white check when on, or a dash when the selection is mixed.
/// Returns the response; `changed()` is set when a click flipped `value`.
pub(super) fn checkbox(ui: &mut egui::Ui, value: &mut bool, indeterminate: bool, label: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::proportional(13.0), theme::TEXT);
    let size = vec2(14.0 + 6.0 + galley.size().x, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let enabled = ui.is_enabled();
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
        let boxr = egui::Rect::from_center_size(egui::pos2(rect.left() + 7.0, rect.center().y), vec2(14.0, 14.0));
        let on = *value && !indeterminate;
        let painter = ui.painter();
        if on || indeterminate {
            painter.rect_filled(boxr, CornerRadius::same(3), fade(theme::ACCENT));
            let glyph = if on { icon::CHECK } else { icon::MINUS };
            painter.text(boxr.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(11.0), Color32::WHITE);
        } else {
            let stroke = if resp.hovered() { theme::WEAK } else { theme::BUTTON_STROKE };
            painter.rect(boxr, CornerRadius::same(3), theme::FIELD, Stroke::new(1.0, fade(stroke)), egui::StrokeKind::Inside);
        }
        let text_pos = egui::pos2(boxr.right() + 6.0, rect.center().y - galley.size().y / 2.0);
        painter.galley(text_pos, galley, fade(theme::TEXT));
    }
    resp
}

/// Size of the stepper's buttons: regular in the inspector, compact in the cue card.
fn stepper_button_size(compact: bool) -> Vec2 {
    if compact { vec2(24.0, 22.0) } else { vec2(26.0, 24.0) }
}

/// One sync-offset step, in seconds.
pub(super) const OFFSET_STEP: f64 = 0.05;

/// The sync-offset stepper: `[−] [value] [+] [reset]`. The value is a drag
/// field that also takes typed input; it turns teal when non-zero. Returns
/// whether `offset` changed. Used by the inspector and the cue card, which edit
/// the same stream field, so both always show the same number.
pub(super) fn offset_stepper(ui: &mut egui::Ui, offset: &mut f64, compact: bool) -> bool {
    let before = *offset;
    let size = stepper_button_size(compact);
    ui.spacing_mut().item_spacing.x = 4.0;
    if ui.add_sized(size, button(icon::MINUS)).on_hover_text("Play earlier by 0.05 s").clicked() {
        *offset -= OFFSET_STEP;
    }
    let color = if offset.abs() >= 0.0005 && compact { theme::CHANGE_OFFSET } else { theme::TEXT };
    let value = egui::DragValue::new(offset)
        .speed(0.005)
        .range(-3600.0..=3600.0)
        .custom_formatter(|v, _| format!("{v:+.3} s"))
        .custom_parser(|s| s.trim().trim_end_matches('s').trim().replace('−', "-").parse().ok());
    let min_w = if compact { 64.0 } else { 70.0 };
    ui.scope(|ui| {
        let v = &mut ui.visuals_mut().widgets;
        for w in [&mut v.inactive, &mut v.hovered, &mut v.active] {
            w.weak_bg_fill = theme::FIELD;
            w.fg_stroke.color = color;
        }
        v.inactive.bg_stroke = Stroke::new(1.0, theme::STROKE);
        ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
        ui.add_sized(vec2(min_w + 14.0, size.y), value)
            .on_hover_text("Drag, or click to type a value in seconds");
    });
    if ui.add_sized(size, button(icon::PLUS)).on_hover_text("Delay by 0.05 s").clicked() {
        *offset += OFFSET_STEP;
    }
    if ui
        .add_sized(size, button(icon::ARROW_COUNTER_CLOCKWISE))
        .on_hover_text("Reset to 0")
        .clicked()
    {
        *offset = 0.0;
    }
    // Snap away float drift from repeated ±0.05 steps, to the millisecond the
    // command actually uses.
    *offset = (*offset * 1000.0).round() / 1000.0;
    *offset != before
}

/// A dashed rounded-rectangle outline (the empty-state drop zone and the drop
/// overlay's target). egui has no dashed stroke for rects, so each side is a
/// dashed line and the corners are left open, which reads the same at 1–2 px.
pub(super) fn dashed_rect(painter: &egui::Painter, rect: egui::Rect, radius: f32, stroke: Stroke) {
    let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let sides = [
        [egui::pos2(l + radius, t), egui::pos2(r - radius, t)],
        [egui::pos2(r, t + radius), egui::pos2(r, b - radius)],
        [egui::pos2(r - radius, b), egui::pos2(l + radius, b)],
        [egui::pos2(l, b - radius), egui::pos2(l, t + radius)],
    ];
    for side in sides {
        painter.extend(egui::Shape::dashed_line(&side, stroke, 6.0, 4.0));
    }
}

/// A one-line message with a leading icon in `color`, truncated to the width.
pub(super) fn status_line(ui: &mut egui::Ui, glyph: &str, text: &str, color: Color32) -> egui::Response {
    ui.label(RichText::new(glyph).size(15.0).color(color));
    line(ui, RichText::new(text).color(color))
}

/// A wrapping message with a leading icon, capped at `max_rows` rows (the rest
/// is cut with an ellipsis), for hints and the ffmpeg banner.
pub(super) fn wrapped_with_icon(ui: &mut egui::Ui, glyph: &str, icon_color: Color32, text: &str, color: Color32, size: f32, max_rows: usize) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(glyph).size(size + 1.0).color(icon_color));
        let mut job = egui::text::LayoutJob::single_section(
            text.to_string(),
            egui::TextFormat::simple(egui::FontId::proportional(size), color),
        );
        job.wrap = egui::text::TextWrapping {
            max_width: ui.available_width(),
            max_rows,
            break_anywhere: false,
            overflow_character: Some('…'),
        };
        ui.add(egui::Label::new(job));
    });
}
