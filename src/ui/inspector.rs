//! The inspector card: the selected rows' fields as an editable form, in the
//! right-hand column. Edits write straight back into `project.streams` — the
//! table rows and inspector are the same objects shown two ways.
//!
//! The card fills its column: a header naming the selection, then TAGS,
//! CONVERSION, SYNC OFFSET (and STREAM DETAILS on wide windows) in a scroll
//! area, and a footer pinned to the bottom with View cues, Extract and Remove.
//! Pinning the footer keeps the actions in one place however long the sections
//! get, including when the cue card halves the column.
//!
//! It works over the whole selection: one selected stream is just the N = 1 case
//! of the batch form. Each field shows the value every selected stream shares,
//! or a "mixed" state (an empty box with a hint, an indeterminate checkbox, a
//! "mixed" combo) when they differ, and an edit writes to all of them. Actions
//! that only make sense for one stream (extract, sync offset, view cues) need
//! exactly one.
//!
//! `language`/`title` map an empty field to `None` (inherit the original tag
//! through the copy). The codec combo switches streams between `Encode::Copy`
//! and `Encode::Audio`. Remove is *soft* for original streams: it sets
//! `stream.removed` (the row stays, dimmed, and `to_args()` skips it) so the
//! change is visible and reversible via Restore. Synthetic (`added`) streams
//! are deleted outright instead.
//!
//! While a run is going the card is shown at 45% and doesn't respond, since the
//! run already captured the project; a preview doesn't lock it.

use super::widgets::{self, badge, helper, section_label};
use super::{InterlaceApp, format_offset, stream_id, theme};
use crate::model::{BITRATE_LADDER, Bitrate, Encode, Kind, Meta, OutStream, Project};
use egui::{Margin, RichText, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;

const CODECS: [&str; 5] = ["aac", "ac3", "opus", "flac", "mp3"];
const CHANNELS: [(Option<u32>, &str); 4] = [
    (None, "source"),
    (Some(1), "1 (mono)"),
    (Some(2), "2 (stereo)"),
    (Some(6), "6 (5.1)"),
];
/// ISO 639-2 codes offered by the language combo, with their English names.
/// Anything else can still be typed into the combo's own field.
const LANGUAGES: [(&str, &str); 30] = [
    ("eng", "English"),
    ("por", "Portuguese"),
    ("spa", "Spanish"),
    ("fre", "French"),
    ("ger", "German"),
    ("ita", "Italian"),
    ("jpn", "Japanese"),
    ("kor", "Korean"),
    ("chi", "Chinese"),
    ("rus", "Russian"),
    ("ara", "Arabic"),
    ("hin", "Hindi"),
    ("pol", "Polish"),
    ("dut", "Dutch"),
    ("swe", "Swedish"),
    ("nor", "Norwegian"),
    ("dan", "Danish"),
    ("fin", "Finnish"),
    ("tur", "Turkish"),
    ("gre", "Greek"),
    ("heb", "Hebrew"),
    ("tha", "Thai"),
    ("vie", "Vietnamese"),
    ("ind", "Indonesian"),
    ("cze", "Czech"),
    ("hun", "Hungarian"),
    ("rum", "Romanian"),
    ("ukr", "Ukrainian"),
    ("cat", "Catalan"),
    ("und", "Undetermined"),
];
/// Width of the TAGS grid's label column.
const LABEL_WIDTH: f32 = 70.0;
/// Gap between sections; a divider sits in the middle of it.
const SECTION_GAP: f32 = 14.0;

/// Mutations the inspector can't apply while it borrows the project, recorded
/// during rendering and applied through `InterlaceApp` afterwards.
#[derive(Default)]
struct Deferred {
    /// Extract the single selected stream.
    extract: bool,
    /// Deselect everything (the header's ×).
    close: bool,
    /// Add a converted sibling after each of these rows.
    add_converted: Option<Vec<usize>>,
    /// Remove the selection (soft for originals, hard for added streams).
    remove: bool,
    /// Undo the soft removal of the selection.
    restore: bool,
    /// Open the cue card on the single selected subtitle stream.
    view_cues: bool,
    /// Close the cue card (the button reads "Hide cues" while it shows this stream).
    hide_cues: bool,
}

pub(super) fn show(ui: &mut Ui, app: &mut InterlaceApp, height: f32, wide: bool) {
    let mut deferred = Deferred::default();
    let rows = app.selection.to_vec();
    let locked = app.run_state.is_main_running();
    let cues_on_selected = match rows.as_slice() {
        [idx] => app.cues_show(*idx),
        _ => false,
    };

    widgets::card_fill(ui, height, Margin::same(12), |ui| {
        if locked {
            // Fade once (not again per disabled widget) and stop interaction.
            ui.style_mut().visuals.disabled_alpha = 1.0;
            ui.disable();
            ui.set_opacity(0.45);
        }
        let Some(project) = app.project.as_mut().filter(|_| !rows.is_empty()) else {
            ui.add(
                egui::Label::new(
                    RichText::new("Load a file, then select a stream to edit its tags, conversion and sync.")
                        .size(12.5)
                        .color(theme::WEAK),
                )
                .wrap(),
            );
            return;
        };
        if rows.iter().any(|&i| i >= project.streams.len()) {
            return;
        }

        header(ui, project, &rows, &mut deferred);
        ui.add_space(SECTION_GAP);

        // The footer is laid out bottom-up first, so the sections get exactly
        // the height left between the header and it.
        // The body, footer and sections each get an explicit id. Automatic ids
        // count the widgets drawn before them, and both the header (a removal
        // warning) and the footer (View cues on a subtitle) vary by stream, so
        // every widget below would be renumbered on a selection change. An
        // `id_salt` isn't enough: egui still mixes in the parent's count.
        let scope = |name: &str, layout| egui::UiBuilder::new().id(egui::Id::new(name)).layout(layout);
        ui.scope_builder(scope("inspector_body", egui::Layout::bottom_up(egui::Align::Min)), |ui| {
            ui.scope_builder(scope("inspector_footer", egui::Layout::bottom_up(egui::Align::Min)), |ui| {
                footer(ui, project, &rows, cues_on_selected, &mut deferred);
            });
            ui.add_space(12.0);
            ui.scope_builder(scope("inspector_sections", egui::Layout::top_down(egui::Align::Min)), |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        sections(ui, project, &rows, wide, &mut deferred);
                    });
            });
        });
    });

    if deferred.extract {
        app.extract_selected();
    }
    if let Some(rows) = deferred.add_converted {
        app.add_converted_streams(&rows);
    }
    if deferred.remove {
        app.remove_selected();
    }
    if deferred.restore {
        app.restore_selected();
    }
    if deferred.close {
        app.selection.clear();
    }
    if deferred.hide_cues {
        app.close_cues();
    } else if deferred.view_cues
        && let [idx] = rows.as_slice()
    {
        app.open_cues(*idx);
    }
}

/// The scrolling middle of the card: TAGS, CONVERSION, SYNC OFFSET and, on
/// wide windows, STREAM DETAILS, divided by thin rules.
fn sections(ui: &mut Ui, project: &mut Project, rows: &[usize], wide: bool, deferred: &mut Deferred) {
    tags_section(ui, project, rows);
    conversion_section(ui, project, rows, deferred);
    offset_section(ui, project, rows);
    if wide && let [idx] = rows {
        details_section(ui, project, *idx);
    }
}

/// A 1 px rule between two sections, centered in the section gap.
fn divider(ui: &mut Ui) {
    ui.add_space(SECTION_GAP);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::SECTION_DIVIDER);
    ui.add_space(SECTION_GAP);
}

/// The value every item agrees on, or `None` when they differ (or there are
/// none). Drives each field's "mixed" state.
fn common<T: PartialEq>(mut values: impl Iterator<Item = T>) -> Option<T> {
    let first = values.next()?;
    values.all(|v| v == first).then_some(first)
}

/// Mutable references to `streams[rows]`, for writing one edit to every
/// selected stream. Walking `iter_mut` hands out disjoint borrows, which plain
/// indexing in a loop can't.
fn pick<'a>(streams: &'a mut [OutStream], rows: &[usize]) -> Vec<&'a mut OutStream> {
    streams
        .iter_mut()
        .enumerate()
        .filter(|(i, _)| rows.contains(i))
        .map(|(_, s)| s)
        .collect()
}

/// The selected rows that aren't marked for removal: the ones edits apply to.
/// When every row is removed, all of them are returned so the (disabled) form
/// still shows their values.
fn live_rows(project: &Project, rows: &[usize]) -> (Vec<usize>, bool) {
    let live: Vec<usize> = rows
        .iter()
        .copied()
        .filter(|&i| !project.streams[i].removed)
        .collect();
    if live.is_empty() {
        (rows.to_vec(), false)
    } else {
        (live, true)
    }
}

/// The header: type badge and stream id (or the count for a multi-selection),
/// the close ×, where it comes from, and a removal warning.
fn header(ui: &mut Ui, project: &Project, rows: &[usize], deferred: &mut Deferred) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            deferred.close = widgets::close_x(ui, 20.0, "Deselect");
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| match rows {
                [idx] => {
                    badge(ui, project.streams[*idx].source.kind);
                    ui.label(RichText::new(stream_id(project, *idx)).font(theme::medium_font(14.0)));
                }
                _ => {
                    ui.label(RichText::new(format!("{} streams", rows.len())).font(theme::medium_font(14.0)));
                }
            });
        });
    });
    ui.add_space(4.0);
    let source = match rows {
        [idx] => {
            let s = &project.streams[*idx];
            let name = super::file_name_lossy(&project.inputs[s.source.input].path).unwrap_or_default();
            format!("from input {} · {name}", s.source.input)
        }
        _ => kind_breakdown(project, rows),
    };
    widgets::line(ui, RichText::new(source).size(12.0).color(theme::WEAK));

    let removed = rows.iter().filter(|&&i| project.streams[i].removed).count();
    let warning = match (rows.len(), removed) {
        (_, 0) => None,
        (1, _) => Some("This stream will be removed from the output.".to_string()),
        (n, r) if r == n => Some(format!("These {n} streams will be removed from the output.")),
        (n, r) => Some(format!("{r} of {n} streams are marked for removal.")),
    };
    if let Some(w) = warning {
        ui.add_space(4.0);
        widgets::line(ui, RichText::new(widgets::with_icon(icon::WARNING, &w)).size(12.0).color(theme::ERROR));
    }
}

/// "2 audio, 1 subtitle": what a multi-selection is made of, in kind order.
fn kind_breakdown(project: &Project, rows: &[usize]) -> String {
    let kinds = [Kind::Video, Kind::Audio, Kind::Subtitle, Kind::Attachment, Kind::Data];
    kinds
        .iter()
        .filter_map(|&k| {
            let n = rows.iter().filter(|&&i| project.streams[i].source.kind == k).count();
            (n > 0).then(|| format!("{n} {}", widgets::kind_text(k)))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A TAGS grid row: the fixed-width label, then the control.
fn grid_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.allocate_ui_with_layout(vec2(LABEL_WIDTH, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_size(vec2(LABEL_WIDTH, 24.0));
            ui.label(RichText::new(label).color(theme::WEAK));
        });
        ui.spacing_mut().item_spacing.x = 6.0;
        add(ui);
    });
}

/// Language, title, and default/forced, each showing the shared value or a mixed
/// state, and writing an edit to every live selected stream.
fn tags_section(ui: &mut Ui, project: &mut Project, rows: &[usize]) {
    let (rows, editable) = live_rows(project, rows);
    // Video has no spoken language, and `forced` is a subtitle concept (foreign
    // dialogue/signs) that no player acts on for a video track — so both fields
    // skip video streams, and are hidden when only video is selected. The
    // underlying `Meta` still carries any probed tags, which pass through
    // untouched on copy; we just don't offer to edit them.
    let non_video: Vec<usize> = rows
        .iter()
        .copied()
        .filter(|&i| project.streams[i].source.kind != Kind::Video)
        .collect();

    section_label(ui, "TAGS");
    ui.add_space(8.0);
    ui.add_enabled_ui(editable, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        if !non_video.is_empty() {
            let shared = common(non_video.iter().map(|&i| &project.streams[i].meta.language)).cloned();
            grid_row(ui, "Language", |ui| {
                if let Some(lang) = language_combo(ui, shared.as_ref()) {
                    for s in pick(&mut project.streams, &non_video) {
                        s.meta.language = lang.clone();
                    }
                }
            });
        }

        let shared = common(rows.iter().map(|&i| &project.streams[i].meta.title)).cloned();
        grid_row(ui, "Title", |ui| {
            let mut title = shared.clone().flatten().unwrap_or_default();
            let edit = egui::TextEdit::singleline(&mut title)
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(8, 4))
                .hint_text(if shared.is_none() { "mixed" } else { "" });
            if ui.add(edit).changed() {
                for s in pick(&mut project.streams, &rows) {
                    s.meta.title = (!title.is_empty()).then(|| title.clone());
                }
            }
        });

        grid_row(ui, "", |ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            flag_checkbox(ui, project, &rows, "Default", |m| &mut m.default);
            if !non_video.is_empty() {
                flag_checkbox(ui, project, &non_video, "Forced", |m| &mut m.forced);
            }
        });
    });
}

/// The language combo: `eng — English` for known codes, the raw code
/// otherwise, `unset` for none and `mixed` when the selection differs. Its popup
/// lists the common codes and has a field for typing any other. Returns the
/// picked value (`Some(None)` = unset) when one was chosen this frame.
fn language_combo(ui: &mut Ui, current: Option<&Option<String>>) -> Option<Option<String>> {
    let text = match current {
        None => "mixed".to_string(),
        Some(None) => "unset".to_string(),
        Some(Some(code)) => language_label(code),
    };
    let dim = !matches!(current, Some(Some(_)));
    let mut picked = None;
    field_combo(ui, |ui| {
        egui::ComboBox::from_id_salt("language")
            .width(ui.available_width().min(170.0))
            .selected_text(RichText::new(text).color(if dim { theme::WEAK } else { theme::TEXT }))
            .height(320.0)
            .show_ui(ui, |ui| {
                let typed_id = ui.id().with("typed");
                let mut typed: String = ui.data_mut(|d| d.get_temp(typed_id)).unwrap_or_default();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut typed)
                        .hint_text("Other code, e.g. bul")
                        .desired_width(150.0),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !typed.trim().is_empty() {
                    picked = Some(Some(typed.trim().to_lowercase()));
                    typed.clear();
                }
                ui.data_mut(|d| d.insert_temp(typed_id, typed));
                ui.separator();
                if ui.selectable_label(matches!(current, Some(None)), "unset").clicked() {
                    picked = Some(None);
                }
                for (code, _) in LANGUAGES {
                    let on = matches!(current, Some(Some(c)) if c == code);
                    if ui.selectable_label(on, language_label(code)).clicked() {
                        picked = Some(Some(code.to_string()));
                    }
                }
            });
    });
    picked
}

/// `eng — English` for a known code, the code itself otherwise.
fn language_label(code: &str) -> String {
    match LANGUAGES.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)) {
        Some((c, name)) => format!("{c} — {name}"),
        None => code.to_string(),
    }
}

/// Draw combos in this scope as fields (`FIELD` fill, `STROKE` outline) rather
/// than buttons, matching the mockup's selects.
fn field_combo<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = theme::FIELD;
        w.inactive.bg_stroke = Stroke::new(1.0, theme::STROKE);
        w.hovered.weak_bg_fill = theme::FIELD;
        w.active.weak_bg_fill = theme::FIELD;
        w.open.weak_bg_fill = theme::FIELD;
        ui.spacing_mut().button_padding = vec2(8.0, 4.0);
        ui.spacing_mut().interact_size.y = 28.0;
        add(ui)
    })
    .inner
}

/// A disposition checkbox over `rows`: checked/unchecked when they agree,
/// indeterminate when they don't. A click writes the new state to all of them.
fn flag_checkbox(ui: &mut Ui, project: &mut Project, rows: &[usize], label: &str, flag: fn(&mut Meta) -> &mut bool) {
    let shared = common(rows.iter().map(|&i| *flag(&mut project.streams[i].meta)));
    let mut value = shared.unwrap_or(false);
    if widgets::checkbox(ui, &mut value, shared.is_none(), label).changed() {
        for s in pick(&mut project.streams, rows) {
            *flag(&mut s.meta) = value;
        }
    }
}

/// Audio conversion over the selection: codec, bitrate and channels combos in a
/// three-column grid. `copy` keeps the stream as is; bitrate and channels only
/// apply (and are only enabled) once a codec is picked.
fn conversion_section(ui: &mut Ui, project: &mut Project, rows: &[usize], deferred: &mut Deferred) {
    let all_audio = rows.iter().all(|&i| project.streams[i].source.kind == Kind::Audio);
    if !all_audio && rows.len() == 1 {
        return;
    }
    let (rows, editable) = live_rows(project, rows);

    divider(ui);
    section_label(ui, "CONVERSION");
    ui.add_space(8.0);
    if !all_audio {
        helper(ui, "Select only audio streams to convert them together.");
        return;
    }

    ui.add_enabled_ui(editable, |ui| {
        let gap = 8.0;
        let col = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for label in ["Codec", "Bitrate", "Channels"] {
                ui.allocate_ui_with_layout(vec2(col, 16.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_size(vec2(col, 16.0));
                    ui.label(RichText::new(label).color(theme::WEAK));
                });
            }
        });
        ui.add_space(4.0);
        let mut streams = pick(&mut project.streams, &rows);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            field_combo(ui, |ui| {
                convert_combo(ui, &mut streams, col);
                bitrate_combo(ui, &mut streams, col);
                channels_combo(ui, &mut streams, col);
            });
        });
        ui.add_space(8.0);
        helper(ui, "copy = keep the stream as is. Pick a codec to re-encode.");

        // Converting in place drops the original codec; this keeps it and adds
        // a re-encoded sibling that can be ordered and tagged on its own.
        let copies: Vec<usize> = rows
            .iter()
            .copied()
            .filter(|&i| matches!(project.streams[i].encode, Encode::Copy))
            .collect();
        if !copies.is_empty() {
            ui.add_space(6.0);
            let label = if copies.len() == 1 {
                "Keep the original, add a converted copy"
            } else {
                "Keep the originals, add converted copies"
            };
            if ui
                .add(widgets::button(RichText::new(widgets::with_icon(icon::PLUS, label)).size(12.0)))
                .on_hover_text("Adds a re-encoded stream right after each original")
                .clicked()
            {
                deferred.add_converted = Some(copies);
            }
        }
    });
}

/// The codec combo over `streams`: `copy`, a codec, or `mixed`. A pick switches
/// all of them. `copy` isn't offered for a converted sibling (an `added` stream
/// on the main input), which would just duplicate its source; delete it instead.
fn convert_combo(ui: &mut Ui, streams: &mut [&mut OutStream], width: f32) {
    let current = common(streams.iter().map(|s| match &s.encode {
        Encode::Copy => "copy".to_string(),
        Encode::Audio { codec, .. } => codec.clone(),
    }));
    let can_copy = !streams.iter().any(|s| s.added && s.source.input == 0);
    egui::ComboBox::from_id_salt("convert_to")
        .selected_text(current.as_deref().unwrap_or("mixed"))
        .width(width)
        .show_ui(ui, |ui| {
            let options = can_copy.then_some("copy").into_iter().chain(CODECS);
            for opt in options {
                if ui.selectable_label(current.as_deref() == Some(opt), opt).clicked() {
                    for s in streams.iter_mut() {
                        set_codec(s, opt);
                    }
                }
            }
        });
}

/// The bitrate combo over `streams`. Only meaningful for lossy codecs, so it's
/// disabled (showing `—`) unless at least one stream encodes to one, and a pick
/// skips the rest (FLAC and copies keep whatever they had).
fn bitrate_combo(ui: &mut Ui, streams: &mut [&mut OutStream], width: f32) {
    let lossy = |s: &OutStream| matches!(&s.encode, Encode::Audio { codec, .. } if codec != "flac");
    let bitrate_of = |s: &OutStream| match &s.encode {
        Encode::Audio { bitrate, .. } => Some(bitrate.clone()),
        Encode::Copy => None,
    };
    let any_lossy = streams.iter().any(|s| lossy(s));
    let considered: Vec<&OutStream> = streams.iter().map(|s| &**s).filter(|s| lossy(s)).collect();
    let current = common(considered.iter().map(|s| bitrate_of(s))).flatten();
    let text = match &current {
        _ if !any_lossy => "—".to_string(),
        // Show the concrete rung AUTO resolves to, e.g. "auto (192k)", when every
        // stream resolves to the same one; plain "auto" otherwise (or when the
        // source bitrate is unknown and it falls back to default).
        Some(Bitrate::Auto) => match common(considered.iter().map(|s| Bitrate::Auto.resolve(s.source.bitrate_kbps))) {
            Some(Some(v)) => format!("auto ({v}k)"),
            _ => "auto".into(),
        },
        Some(Bitrate::Default) => "default".into(),
        Some(Bitrate::Fixed(b)) => format!("{b}k"),
        None => "mixed".into(),
    };
    let mut pick_rate: Option<Bitrate> = None;
    ui.add_enabled_ui(any_lossy, |ui| {
        egui::ComboBox::from_id_salt("bitrate")
            .selected_text(text)
            .width(width)
            .show_ui(ui, |ui| {
                let options = [Bitrate::Auto, Bitrate::Default]
                    .into_iter()
                    .chain(BITRATE_LADDER.into_iter().map(Bitrate::Fixed));
                for opt in options {
                    let label = match &opt {
                        Bitrate::Auto => "auto".to_string(),
                        Bitrate::Default => "default".to_string(),
                        Bitrate::Fixed(b) => format!("{b}k"),
                    };
                    if ui.selectable_label(current.as_ref() == Some(&opt), label).clicked() {
                        pick_rate = Some(opt);
                    }
                }
            });
    });
    if let Some(rate) = pick_rate {
        for s in streams.iter_mut().filter(|s| lossy(s)) {
            if let Encode::Audio { bitrate, .. } = &mut s.encode {
                *bitrate = rate.clone();
            }
        }
    }
}

/// The channel-layout combo over `streams`: their shared layout, or "mixed".
/// Disabled while every stream is a copy; a pick applies to the converting ones.
fn channels_combo(ui: &mut Ui, streams: &mut [&mut OutStream], width: f32) {
    let converting = streams.iter().any(|s| s.converted());
    let current = common(streams.iter().filter(|s| s.converted()).map(|s| match &s.encode {
        Encode::Audio { channels, .. } => *channels,
        Encode::Copy => None,
    }));
    let text = match current {
        _ if !converting => "source",
        Some(c) => CHANNELS
            .iter()
            .find(|(v, _)| *v == c)
            .map(|(_, label)| *label)
            .unwrap_or("source"),
        None => "mixed",
    };
    ui.add_enabled_ui(converting, |ui| {
        egui::ComboBox::from_id_salt("channels")
            .selected_text(text)
            .width(width)
            .show_ui(ui, |ui| {
                for (value, label) in CHANNELS {
                    if ui.selectable_label(current == Some(value), label).clicked() {
                        for s in streams.iter_mut() {
                            if let Encode::Audio { channels, .. } = &mut s.encode {
                                *channels = value;
                            }
                        }
                    }
                }
            });
    });
}

/// Switch a stream's encode to `codec` ("copy" reverts to stream-copy),
/// preserving any bitrate/channels already chosen.
fn set_codec(stream: &mut OutStream, codec: &str) {
    if codec == "copy" {
        stream.encode = Encode::Copy;
        return;
    }
    let (bitrate, channels) = match &stream.encode {
        Encode::Audio { bitrate, channels, .. } => (bitrate.clone(), *channels),
        Encode::Copy => (Bitrate::Fixed(192), None),
    };
    stream.encode = Encode::Audio {
        codec: codec.to_string(),
        bitrate,
        channels,
    };
}

/// Sync offset for a single audio or subtitle track, whether inserted from an
/// added file or already inside the main file. Video isn't offered: it's the
/// clock everything else syncs against. A shifted track inside the main file
/// costs a second read of that file during the run (see `args.rs`), so a note
/// says so.
fn offset_section(ui: &mut Ui, project: &mut Project, rows: &[usize]) {
    let [idx] = rows else { return };
    let stream = &mut project.streams[*idx];
    if !matches!(stream.source.kind, Kind::Audio | Kind::Subtitle) {
        return;
    }
    divider(ui);
    section_label(ui, "SYNC OFFSET");
    ui.add_space(8.0);
    ui.add_enabled_ui(!stream.removed, |ui| {
        ui.horizontal(|ui| {
            widgets::offset_stepper(ui, &mut stream.offset_secs, false);
            ui.add_space(6.0);
            ui.label(RichText::new("step 0.05 s").size(12.0).color(theme::WEAK));
        });
    });
    ui.add_space(8.0);
    helper(ui, "+ delays this track, − plays it earlier.");
    if stream.source.input == 0 && stream.has_offset() {
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(icon::INFO).size(13.0).color(theme::CHANGE_OFFSET));
            helper(ui, "The run reads the file once more to shift just this track.");
        });
    }
}

/// STREAM DETAILS (wide windows): what's known about the source stream, as a
/// key/value grid.
fn details_section(ui: &mut Ui, project: &Project, idx: usize) {
    let s = &project.streams[idx];
    let mut rows: Vec<(&str, String)> = vec![("Codec", s.source.codec.clone())];
    if let Some(kbps) = s.source.bitrate_kbps {
        rows.push(("Bitrate", format!("{kbps} kb/s")));
    }
    if let Some(d) = project.duration_secs {
        let t = d.round() as u64;
        rows.push(("Duration", format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)));
    }
    rows.push(("Input map", format!("{}:{}", s.source.input, s.source.index)));
    if s.has_offset() {
        rows.push(("Offset", format_offset(s.offset_secs)));
    }

    divider(ui);
    section_label(ui, "STREAM DETAILS");
    ui.add_space(8.0);
    egui::Grid::new("stream_details")
        .num_columns(2)
        .min_col_width(110.0)
        .spacing([0.0, 6.0])
        .show(ui, |ui| {
            for (key, value) in rows {
                ui.label(RichText::new(key).color(theme::WEAK));
                widgets::line(ui, RichText::new(value).font(egui::FontId::monospace(12.5)));
                ui.end_row();
            }
        });
}

/// The pinned footer, laid out bottom-up: Extract and Remove/Restore side by
/// side, with View cues on its own row above them for a subtitle.
fn footer(ui: &mut Ui, project: &Project, rows: &[usize], cues_on: bool, deferred: &mut Deferred) {
    let width = ui.available_width();
    let gap = 8.0;
    let half = ((width - gap) / 2.0).floor();
    let streams: Vec<&OutStream> = rows.iter().map(|&i| &project.streams[i]).collect();

    // `bottom_up` stacks from the bottom, and a horizontal inside it still runs
    // left to right.
    ui.allocate_ui_with_layout(vec2(width, 30.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let (can_extract, why) = match streams.as_slice() {
            [s] if s.added => (false, "Added streams aren't in the source file"),
            [s] if s.removed => (false, "Restore the stream to extract it"),
            [s] if matches!(s.source.kind, Kind::Attachment | Kind::Data) => {
                (false, "Attachments and data streams can't be extracted here")
            }
            [_] => (true, "Copy just this stream out to its own file"),
            _ => (false, "Select one stream to extract it"),
        };
        let extract = ui.add_enabled_ui(can_extract, |ui| {
            ui.add_sized(vec2(half, 30.0), widgets::button(widgets::with_icon(icon::EXPORT, "Extract to file…")))
        });
        let extract = extract.inner;
        let extract = if can_extract { extract.on_hover_text(why) } else { extract.on_disabled_hover_text(why) };
        deferred.extract = extract.clicked();

        // Added streams are never soft-removed, so "all removed" means every
        // stream is an original awaiting removal: the only case for Restore.
        let all_removed = streams.iter().all(|s| s.removed);
        if all_removed {
            let restore = widgets::button(widgets::with_icon(icon::ARROW_COUNTER_CLOCKWISE, "Restore"));
            deferred.restore = ui
                .add_sized(vec2(half, 30.0), restore)
                .on_hover_text("Keep in the output")
                .clicked();
        } else {
            let hover = match streams.as_slice() {
                [s] if s.added => "Delete this added stream",
                [_] => "Drop this stream from the output (Restore undoes it)",
                _ => "Drop these streams from the output (added streams are deleted)",
            };
            deferred.remove = ui
                .add_sized(vec2(half, 30.0), widgets::danger_button(&widgets::with_icon(icon::TRASH, "Remove")))
                .on_hover_text(hover)
                .clicked();
        }
    });

    let [s] = streams.as_slice() else { return };
    if s.source.kind != Kind::Subtitle {
        return;
    }
    ui.add_space(gap);
    let text = crate::subs::is_text_subtitle(&s.source.codec);
    if !text {
        // Laid out bottom-up, so the note goes in before the button above it.
        ui.add(
            egui::Label::new(
                RichText::new(format!("{} subtitles are images, so there's no text to list.", image_format(&s.source.codec)))
                    .size(12.0)
                    .color(theme::WEAK),
            )
            .wrap(),
        );
        ui.add_space(4.0);
    }
    let label = widgets::with_icon(icon::LIST_MAGNIFYING_GLASS, if cues_on { "Hide cues" } else { "View cues" });
    let btn = if cues_on { widgets::active_button(label) } else { widgets::button(label) };
    let resp = ui.add_enabled(text, |ui: &mut Ui| ui.add_sized(vec2(width, 30.0), btn));
    if resp.on_hover_text("List this subtitle's lines and timings").clicked() {
        if cues_on {
            deferred.hide_cues = true;
        } else {
            deferred.view_cues = true;
        }
    }
}

/// The everyday name of an image-based subtitle codec, for the View cues note.
fn image_format(codec: &str) -> String {
    match codec {
        "hdmv_pgs_subtitle" | "pgssub" => "PGS".into(),
        "dvd_subtitle" | "dvdsub" => "VobSub".into(),
        "dvb_subtitle" | "dvbsub" => "DVB".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_reports_shared_value_or_none() {
        assert_eq!(common([1, 1, 1].into_iter()), Some(1));
        assert_eq!(common([1, 2].into_iter()), None);
        assert_eq!(common(std::iter::empty::<i32>()), None);
    }

    #[test]
    fn language_label_names_known_codes_only() {
        assert_eq!(language_label("eng"), "eng — English");
        assert_eq!(language_label("xyz"), "xyz");
    }
}
