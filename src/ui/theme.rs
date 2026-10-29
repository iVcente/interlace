//! The "Midnight" look: color tokens, fonts and the egui `Style` they feed.
//!
//! Every color in the UI comes from a named role here (panel, field, accent,
//! kind/change colors…), so a light set could be added later by swapping the
//! constants for a palette struct. Nothing outside this module should spell a
//! raw `Color32`.
//!
//! Fonts: Ubuntu Regular/Medium are embedded (egui ships only Ubuntu-Light),
//! Phosphor icons come from `egui-phosphor`, and Hack stays the monospace face.
//! Ubuntu has no CJK, Arabic, Hebrew or Thai glyphs, which subtitle cues often
//! need, so Windows system fonts are appended as fallbacks when present. They're
//! read from disk rather than bundled (Noto would add ≈30 MB to the binary).

use egui::{Color32, FontFamily, FontId, TextStyle};

/// Window background and the gutters between cards.
pub(crate) const BG: Color32 = Color32::from_rgb(0x0E, 0x11, 0x17);
/// Card fill.
pub(crate) const PANEL: Color32 = Color32::from_rgb(0x14, 0x19, 0x24);
/// Elevated surfaces: chips, toasts, overlay chips, hovered cue rows.
pub(crate) const CARD: Color32 = Color32::from_rgb(0x1A, 0x20, 0x30);
/// Text edits, the command box and progress-bar tracks.
pub(crate) const FIELD: Color32 = Color32::from_rgb(0x0A, 0x0D, 0x13);
/// Card and field borders.
pub(crate) const STROKE: Color32 = Color32::from_rgb(0x27, 0x30, 0x47);
/// Button fill.
pub(crate) const BUTTON: Color32 = Color32::from_rgb(0x1D, 0x25, 0x36);
/// Button stroke, and the chip index badge fill.
pub(crate) const BUTTON_STROKE: Color32 = Color32::from_rgb(0x2E, 0x3A, 0x54);
/// Divider between table rows.
pub(crate) const ROW_DIVIDER: Color32 = Color32::from_rgb(0x1B, 0x22, 0x32);
/// Divider between inspector sections.
pub(crate) const SECTION_DIVIDER: Color32 = Color32::from_rgb(0x1D, 0x25, 0x36);
/// Drag handles and disabled text.
pub(crate) const DIM: Color32 = Color32::from_rgb(0x5B, 0x66, 0x80);
/// Normal text.
pub(crate) const TEXT: Color32 = Color32::from_rgb(0xEE, 0xF2, 0xF8);
/// Weak text: labels, helpers, secondary values.
pub(crate) const WEAK: Color32 = Color32::from_rgb(0x8D, 0x98, 0xAD);
/// Run, focus and selection marks.
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x4C, 0x9B, 0xFF);
/// Text on an `ACCENT` fill.
pub(crate) const ON_ACCENT: Color32 = Color32::from_rgb(0x06, 0x10, 0x1F);
/// Selected table row (and selected cue, active toggle buttons).
pub(crate) const SELECTED: Color32 = Color32::from_rgb(0x17, 0x38, 0x68);
/// Removed table row.
pub(crate) const REMOVED: Color32 = Color32::from_rgb(0x33, 0x18, 0x22);
pub(crate) const SUCCESS: Color32 = Color32::from_rgb(0x3C, 0xCB, 0x7F);
pub(crate) const WARNING: Color32 = Color32::from_rgb(0xF2, 0xB4, 0x41);
pub(crate) const ERROR: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x6B);
/// Error text on an error-tinted fill (the banner, error boxes).
pub(crate) const ERROR_TEXT: Color32 = Color32::from_rgb(0xF6, 0xB9, 0xBE);
/// Error banner and error box stroke.
pub(crate) const ERROR_STROKE: Color32 = Color32::from_rgb(0x5A, 0x26, 0x33);
/// The outlined Remove button's stroke.
pub(crate) const REMOVE_STROKE: Color32 = Color32::from_rgb(0x8F, 0x33, 0x40);
/// The outlined Remove button's text.
pub(crate) const REMOVE_TEXT: Color32 = Color32::from_rgb(0xFF, 0x8F, 0x8F);
/// Command text.
pub(crate) const CODE: Color32 = Color32::from_rgb(0xC8, 0xD2, 0xE4);

/// Kind colors (type badges, drop-overlay chips).
pub(crate) const KIND_VIDEO: Color32 = Color32::from_rgb(0x3B, 0x7B, 0xFF);
pub(crate) const KIND_AUDIO: Color32 = Color32::from_rgb(0x8B, 0x5C, 0xF6);
pub(crate) const KIND_SUBTITLE: Color32 = Color32::from_rgb(0xF5, 0x9E, 0x0B);
pub(crate) const KIND_OTHER: Color32 = Color32::from_rgb(0x5B, 0x66, 0x80);

/// Change-pill colors in the table's CHANGES column.
pub(crate) const CHANGE_REMOVE: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
pub(crate) const CHANGE_CONVERT: Color32 = Color32::from_rgb(0x8B, 0x5C, 0xF6);
pub(crate) const CHANGE_RETAG: Color32 = Color32::from_rgb(0xD9, 0x8E, 0x04);
pub(crate) const CHANGE_FLAGS: Color32 = Color32::from_rgb(0x3B, 0x7B, 0xFF);
pub(crate) const CHANGE_EMBED: Color32 = Color32::from_rgb(0x22, 0xB5, 0x67);
pub(crate) const CHANGE_OFFSET: Color32 = Color32::from_rgb(0x14, 0xB8, 0xA6);

/// Gutter between cards, and the outer window margin.
pub(crate) const GUTTER: f32 = 8.0;

/// The font family for Ubuntu Medium (headers, the Run button, titles).
pub(crate) fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}

/// A Ubuntu Medium font of `size` points.
pub(crate) fn medium_font(size: f32) -> FontId {
    FontId::new(size, medium())
}

/// `color` at `alpha` (0–255), for tints such as drop-target fills. `Color32` is
/// premultiplied, so this goes through the unmultiplied constructor.
pub(crate) fn tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Install fonts and style on `ctx`. `system_fallbacks` adds the Windows CJK /
/// Arabic / Thai fonts; tests pass `false` to stay fast and machine-independent.
pub(crate) fn install(ctx: &egui::Context, system_fallbacks: bool) {
    ctx.set_fonts(fonts(system_fallbacks));
    ctx.global_style_mut(style);
}

/// Font definitions: Ubuntu Regular as the body face, a `medium` family,
/// Phosphor icons mixed into both, and optional system fallbacks last.
fn fonts(system_fallbacks: bool) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Ubuntu-Regular".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Ubuntu-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "Ubuntu-Medium".into(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Ubuntu-Medium.ttf")).into(),
    );
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    let fallbacks = if system_fallbacks { system_fonts(&mut fonts) } else { Vec::new() };

    // Proportional: Ubuntu Regular first, then (from add_to_fonts) Phosphor, then
    // egui's defaults (Ubuntu-Light, emoji), then the system fallbacks.
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "Ubuntu-Regular".into());
    proportional.extend(fallbacks.iter().cloned());
    let mut medium = proportional.clone();
    medium.insert(0, "Ubuntu-Medium".into());
    fonts.families.insert(self::medium(), medium);
    // Monospace keeps Hack, but can still show cue text in any script.
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.push("phosphor".into());
    mono.extend(fallbacks);
    fonts
}

/// Load whichever Windows fallback fonts exist, returning their keys in
/// priority order. Missing files are skipped; this never fails.
fn system_fonts(fonts: &mut egui::FontDefinitions) -> Vec<String> {
    // Segoe UI covers Arabic, Hebrew and more Latin/Cyrillic; then Chinese,
    // Japanese, Korean and Thai.
    const FILES: [&str; 5] = ["segoeui.ttf", "msyh.ttc", "YuGothR.ttc", "malgun.ttf", "LeelawUI.ttf"];
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let mut keys = Vec::new();
    for file in FILES {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts
                .font_data
                .insert(file.into(), egui::FontData::from_owned(bytes).into());
            keys.push(file.to_string());
        }
    }
    keys
}

/// Apply the Midnight tokens, text sizes and spacing to egui's style.
fn style(style: &mut egui::Style) {
    use egui::{CornerRadius, Margin, Stroke, vec2};

    style.text_styles = [
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Body, FontId::proportional(13.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Heading, FontId::new(14.0, medium())),
        (TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into();

    // Labels sit inside clickable rows and cards; selectable text would swallow
    // those clicks (a selectable label senses drags for text selection).
    style.interaction.selectable_labels = false;
    // Panel resize grab zones straddle the panel edge; wide enough to cover
    // the middle of the 8 px gutter, where the resize handle is drawn.
    style.interaction.resize_grab_radius_side = 6.0;

    let s = &mut style.spacing;
    s.item_spacing = vec2(6.0, 6.0);
    s.button_padding = vec2(10.0, 4.0);
    s.interact_size = vec2(24.0, 24.0);
    s.window_margin = Margin::same(16);
    s.combo_width = 120.0;
    s.icon_width = 14.0;
    s.icon_width_inner = 8.0;
    s.icon_spacing = 6.0;

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, STROKE);
    v.window_corner_radius = CornerRadius::same(10);
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.menu_corner_radius = CornerRadius::same(6);
    v.extreme_bg_color = FIELD;
    v.text_edit_bg_color = Some(FIELD);
    v.faint_bg_color = CARD;
    v.code_bg_color = FIELD;
    v.weak_text_color = Some(WEAK);
    v.warn_fg_color = WARNING;
    v.error_fg_color = ERROR;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = SELECTED;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.disabled_alpha = 0.4;

    let radius = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, SECTION_DIVIDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = FIELD;
    w.inactive.weak_bg_fill = BUTTON;
    w.inactive.bg_stroke = Stroke::new(1.0, BUTTON_STROKE);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = radius;
    w.inactive.expansion = 0.0;

    let hover_fill = Color32::from_rgb(0x24, 0x2E, 0x43);
    w.hovered.bg_fill = FIELD;
    w.hovered.weak_bg_fill = hover_fill;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3A, 0x48, 0x66));
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = FIELD;
    w.active.weak_bg_fill = Color32::from_rgb(0x2A, 0x37, 0x52);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, TEXT);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open = w.active;
    w.open.weak_bg_fill = hover_fill;
}
