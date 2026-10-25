//! The inspector: the selected rows' fields as an editable form, shown in the
//! right-hand side panel. Edits write straight back into `project.streams` —
//! the table rows and inspector are the same objects shown two ways.
//!
//! It works over the whole selection: one selected stream is just the N = 1 case
//! of the batch form. Each field shows the value every selected stream shares,
//! or a "mixed" state (an empty box with a hint, an indeterminate checkbox, a
//! "mixed" combo) when they differ, and an edit writes to all of them. Actions
//! that only make sense for one stream (extract, sync offset) need exactly one.
//!
//! `language`/`title` map an empty field to `None` (inherit the original tag
//! through the copy). The convert controls switch streams between `Encode::Copy`
//! and `Encode::Audio` and tune codec/bitrate/channels. The Remove button is
//! *soft* for original streams: it sets `stream.removed` (the row stays, dimmed,
//! and `to_args()` skips it) so the change is visible and reversible via Restore.
//! Synthetic (`added`) streams are deleted outright instead.

use super::{InterlaceApp, card, section_label};
use crate::model::{BITRATE_LADDER, Bitrate, Encode, Kind, Meta, OutStream, Project};

const CODECS: [&str; 5] = ["aac", "ac3", "opus", "flac", "mp3"];
const CHANNELS: [(Option<u32>, &str); 4] = [
    (None, "source"),
    (Some(1), "1 (mono)"),
    (Some(2), "2 (stereo)"),
    (Some(6), "6 (5.1)"),
];
const RED: egui::Color32 = egui::Color32::from_rgb(220, 80, 80);
const GREEN: egui::Color32 = egui::Color32::from_rgb(46, 160, 90);

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
}

/// A fixed 22×22 close button whose `×` is painted at the exact rect center
/// (egui's `Button` can't perfectly center a lone glyph). The fixed footprint
/// also stops hover expansion from reflowing the header. Returns `true` on click.
fn close_button(ui: &mut egui::Ui) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let resp = resp.on_hover_text("Close inspector");
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(4), v.weak_bg_fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(16.0),
            v.fg_stroke.color,
        );
    }
    resp.clicked()
}

pub(super) fn show(ui: &mut egui::Ui, app: &mut InterlaceApp) {
    let mut deferred = Deferred::default();
    let rows = app.selection.to_vec();

    card(ui, |ui| {
        if rows.is_empty() {
            section_label(ui, "INSPECTOR");
            ui.add_space(4.0);
            ui.weak("Select a stream to edit its language, title, flags, and conversion.");
            ui.add_space(4.0);
            ui.weak("Ctrl-click or Shift-click rows to edit several at once.");
            return;
        }
        let Some(project) = app.project.as_mut() else { return };
        if rows.iter().any(|&i| i >= project.streams.len()) {
            return;
        }

        header(ui, project, &rows, &mut deferred);
        tags_section(ui, project, &rows);
        conversion_section(ui, project, &rows, &mut deferred);
        actions_section(ui, project, &rows, &mut deferred);
        offset_section(ui, project, &rows);
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

/// Title row (what's selected, plus the close ×) and a removal warning.
fn header(ui: &mut egui::Ui, project: &Project, rows: &[usize], deferred: &mut Deferred) {
    let title = match rows {
        [idx] => {
            let kind = project.streams[*idx].source.kind;
            let type_rel = project.streams[..*idx]
                .iter()
                .filter(|s| s.source.kind == kind)
                .count();
            format!("INSPECTOR · {} {} (SELECTED)", super::kind_text(kind).to_uppercase(), type_rel)
        }
        _ => format!("INSPECTOR · {} STREAMS SELECTED", rows.len()),
    };
    ui.horizontal(|ui| {
        section_label(ui, &title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if close_button(ui) {
                deferred.close = true;
            }
        });
    });
    if rows.len() > 1 {
        ui.weak(kind_breakdown(project, rows));
    }
    ui.add_space(6.0);

    let removed = rows.iter().filter(|&&i| project.streams[i].removed).count();
    let warning = match (rows.len(), removed) {
        (_, 0) => None,
        (1, _) => Some("⚠ This stream will be removed from the output.".to_string()),
        (n, r) if r == n => Some(format!("⚠ These {n} streams will be removed from the output.")),
        (n, r) => Some(format!("⚠ {r} of {n} streams are marked for removal.")),
    };
    if let Some(w) = warning {
        ui.colored_label(RED, w);
        ui.add_space(6.0);
    }
}

/// "2 audio, 1 subtitle" — what a multi-selection is made of, in kind order.
fn kind_breakdown(project: &Project, rows: &[usize]) -> String {
    let kinds = [Kind::Video, Kind::Audio, Kind::Subtitle, Kind::Attachment, Kind::Data];
    kinds
        .iter()
        .filter_map(|&k| {
            let n = rows.iter().filter(|&&i| project.streams[i].source.kind == k).count();
            (n > 0).then(|| format!("{n} {}", super::kind_text(k)))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Language, title, and default/forced, each showing the shared value or a mixed
/// state, and writing an edit to every live selected stream.
fn tags_section(ui: &mut egui::Ui, project: &mut Project, rows: &[usize]) {
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

    ui.add_enabled_ui(editable, |ui| {
        if !non_video.is_empty() {
            let shared = common(non_video.iter().map(|&i| &project.streams[i].meta.language)).cloned();
            field(ui, "language", |ui| {
                let mut lang = shared.clone().flatten().unwrap_or_default();
                let edit = egui::TextEdit::singleline(&mut lang)
                    .desired_width(84.0)
                    .hint_text(if shared.is_none() { "mixed" } else { "" });
                if ui.add(edit).changed() {
                    let lang = lang.trim().to_string();
                    for s in pick(&mut project.streams, &non_video) {
                        s.meta.language = (!lang.is_empty()).then(|| lang.clone());
                    }
                }
            });
            ui.add_space(4.0);
        }

        let shared = common(rows.iter().map(|&i| &project.streams[i].meta.title)).cloned();
        field(ui, "title", |ui| {
            let mut title = shared.clone().flatten().unwrap_or_default();
            let edit = egui::TextEdit::singleline(&mut title)
                .desired_width(220.0)
                .hint_text(if shared.is_none() { "mixed" } else { "" });
            if ui.add(edit).changed() {
                for s in pick(&mut project.streams, &rows) {
                    s.meta.title = (!title.is_empty()).then(|| title.clone());
                }
            }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            flag_checkbox(ui, project, &rows, "default", |m| &mut m.default);
            if !non_video.is_empty() {
                flag_checkbox(ui, project, &non_video, "forced", |m| &mut m.forced);
            }
        });
    });
}

/// A disposition checkbox over `rows`: checked/unchecked when they agree,
/// indeterminate when they don't. A click writes the new state to all of them.
fn flag_checkbox(
    ui: &mut egui::Ui,
    project: &mut Project,
    rows: &[usize],
    label: &str,
    flag: fn(&mut Meta) -> &mut bool,
) {
    let shared = common(rows.iter().map(|&i| *flag(&mut project.streams[i].meta)));
    let mut value = shared.unwrap_or(false);
    let checkbox = egui::Checkbox::new(&mut value, label).indeterminate(shared.is_none());
    if ui.add(checkbox).changed() {
        for s in pick(&mut project.streams, rows) {
            *flag(&mut s.meta) = value;
        }
    }
}

/// Audio conversion over the selection. Only for the file's own streams: an
/// imported track is embedded as-is — re-encoding what you're bringing in
/// doesn't fit that flow — so the section is hidden if any selected stream is
/// drawn from an added input.
fn conversion_section(ui: &mut egui::Ui, project: &mut Project, rows: &[usize], deferred: &mut Deferred) {
    if rows.iter().any(|&i| project.streams[i].source.input != 0) {
        return;
    }
    let single = rows.len() == 1;
    let (rows, editable) = live_rows(project, rows);

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);
    section_label(ui, "CONVERSION");
    ui.add_space(6.0);

    ui.add_enabled_ui(editable, |ui| {
        if rows.iter().any(|&i| project.streams[i].source.kind != Kind::Audio) {
            ui.weak(if single {
                "Only audio streams can be converted."
            } else {
                "Select only audio streams to convert them together."
            });
            return;
        }
        let (copies, converting): (Vec<usize>, Vec<usize>) =
            rows.iter().partition(|&&i| matches!(project.streams[i].encode, Encode::Copy));

        if converting.is_empty() {
            // All plain copies: offer the two ways to convert.
            ui.weak(if single {
                "Re-encode this audio to another codec.".to_string()
            } else {
                format!("Re-encode these {} audio streams to another codec.", rows.len())
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .button("convert in place")
                    .on_hover_text("Replace with a re-encoded version (drops the original codec)")
                    .clicked()
                {
                    for s in pick(&mut project.streams, &copies) {
                        s.encode = Encode::default_audio();
                    }
                }
                if ui
                    .button("add as new stream")
                    .on_hover_text("Keep the original and add a converted copy you can reorder and tag")
                    .clicked()
                {
                    deferred.add_converted = Some(copies.clone());
                }
            });
            return;
        }

        if !copies.is_empty() {
            // Some copy, some convert: say so, and offer to convert the rest. The
            // combos below tune only the ones already converting.
            ui.weak(format!("{} of {} streams are converting.", converting.len(), rows.len()));
            ui.add_space(6.0);
            if ui
                .button("convert the rest")
                .on_hover_text("Also re-encode the selected streams that are still copies")
                .clicked()
            {
                for s in pick(&mut project.streams, &copies) {
                    s.encode = Encode::default_audio();
                }
            }
            ui.add_space(6.0);
        }

        let mut streams = pick(&mut project.streams, &converting);
        ui.horizontal(|ui| {
            field(ui, "codec", |ui| convert_combo(ui, &mut streams));
            field(ui, "bitrate", |ui| bitrate_combo(ui, &mut streams));
            field(ui, "channels", |ui| channels_combo(ui, &mut streams));
        });
        // An original stream can drop back to stream-copy; a synthetic added copy
        // can't (that would just duplicate its source) — it's removed outright in
        // the actions section instead.
        if streams.iter().any(|s| !s.added) {
            ui.add_space(6.0);
            if ui
                .button("revert to copy")
                .on_hover_text("Stop converting; stream-copy the original")
                .clicked()
            {
                for s in streams.iter_mut().filter(|s| !s.added) {
                    s.encode = Encode::Copy;
                }
            }
        }
    });
}

/// Remove/restore for the whole selection, then the single-stream extract row.
fn actions_section(ui: &mut egui::Ui, project: &Project, rows: &[usize], deferred: &mut Deferred) {
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    let streams: Vec<&OutStream> = rows.iter().map(|&i| &project.streams[i]).collect();
    // Added streams are never soft-removed, so "all removed" means every stream
    // is an original awaiting removal — the only case where Restore applies.
    let all_removed = streams.iter().all(|s| s.removed);
    let noun = match rows.len() {
        1 => "stream".to_string(),
        n => format!("{n} streams"),
    };
    let (label, fill, hover) = match (streams.as_slice(), all_removed) {
        ([s], _) if s.added => (format!("🗑 remove {noun}"), RED, "Delete this converted stream"),
        (_, true) => (format!("↺ restore {noun}"), GREEN, "Keep in the output"),
        ([_], false) => (format!("🗑 remove {noun}"), RED, "Drop this stream from the output (reversible)"),
        (_, false) => (
            format!("🗑 remove {noun}"),
            RED,
            "Drop from the output (reversible; added converted streams are deleted)",
        ),
    };
    let btn = egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE)).fill(fill);
    if ui
        .add_sized(egui::vec2(ui.available_width(), 26.0), btn)
        .on_hover_text(hover)
        .clicked()
    {
        if all_removed {
            deferred.restore = true;
        } else {
            deferred.remove = true;
        }
    }

    // Extract: one original stream at a time (a synthetic stream isn't in the
    // source file, so there's nothing to extract).
    match rows {
        [idx] if !streams[0].added => extract_row(ui, project, *idx, deferred),
        [_] => {}
        _ => {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Button::new("extract to file…"));
                ui.add(egui::Label::new(egui::RichText::new("select one stream to extract").weak()).truncate());
            });
        }
    }
}

/// The extract button for `streams[idx]` with a preview of its output name.
fn extract_row(ui: &mut egui::Ui, project: &Project, idx: usize, deferred: &mut Deferred) {
    let stream = &project.streams[idx];
    let kind = stream.source.kind;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let can_extract = !stream.removed && !matches!(kind, Kind::Attachment | Kind::Data);
        if ui
            .add_enabled(can_extract, egui::Button::new("extract to file…"))
            .on_hover_text("Copy just this stream out to its own file")
            .clicked()
        {
            deferred.extract = true;
        }
        match project.extract(idx) {
            Some(x) => {
                let name = super::file_name_lossy(&x.output).unwrap_or_default();
                // Truncate to the space left by the button: a long output name
                // must not widen the inspector panel past the window (the panel
                // grows to fit its content, then overflows). A truncated Label
                // shows the full text on hover by itself.
                ui.add(egui::Label::new(egui::RichText::new(format!("» {name}")).weak()).truncate());
            }
            None => {
                ui.add(
                    egui::Label::new(egui::RichText::new("attachments/data can't be extracted here").weak())
                        .truncate(),
                );
            }
        }
    });
}

/// Sync offset for a single embedded track. Only added inputs (index > 0) can be
/// shifted; the primary is the clock everything else syncs against. It's an
/// input-level setting, so it's offered only when exactly one stream is selected.
fn offset_section(ui: &mut egui::Ui, project: &mut Project, rows: &[usize]) {
    let [idx] = rows else { return };
    let input_idx = project.streams[*idx].source.input;
    if input_idx == 0 {
        return;
    }
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);
    section_label(ui, "SYNC OFFSET");
    ui.add_space(6.0);
    let input = &mut project.inputs[input_idx];
    ui.horizontal(|ui| {
        if ui.button("−").on_hover_text("Advance 0.05 s").clicked() {
            input.offset_secs -= 0.05;
        }
        ui.add(
            egui::DragValue::new(&mut input.offset_secs)
                .suffix(" s")
                .speed(0.01)
                .max_decimals(3)
                .range(-3600.0..=3600.0),
        );
        if ui.button("+").on_hover_text("Delay 0.05 s").clicked() {
            input.offset_secs += 0.05;
        }
        if ui.button("⟲").on_hover_text("Reset to 0").clicked() {
            input.offset_secs = 0.0;
        }
    });
    ui.add_space(4.0);
    ui.weak("+ delays this track, − advances it.");
}

/// A captioned field: small label above its widget, matching the mockup.
fn field(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .small()
                .color(egui::Color32::from_gray(150)),
        );
        add(ui);
    });
}

/// The target-codec combo over converting `streams`: their shared codec, or
/// "mixed". A pick switches all of them.
fn convert_combo(ui: &mut egui::Ui, streams: &mut [&mut OutStream]) {
    let current = common(streams.iter().map(|s| match &s.encode {
        Encode::Copy => "copy".to_string(),
        Encode::Audio { codec, .. } => codec.clone(),
    }));
    egui::ComboBox::from_id_salt("convert_to")
        .selected_text(current.as_deref().unwrap_or("mixed"))
        .width(84.0)
        .show_ui(ui, |ui| {
            for opt in CODECS {
                if ui.selectable_label(current.as_deref() == Some(opt), opt).clicked() {
                    for s in streams.iter_mut() {
                        set_codec(s, opt);
                    }
                }
            }
        });
}

/// The bitrate combo over converting `streams`. Only meaningful for lossy
/// codecs, so it's disabled when every stream encodes FLAC, and a pick skips the
/// FLAC ones (which keep whatever they had).
fn bitrate_combo(ui: &mut egui::Ui, streams: &mut [&mut OutStream]) {
    let lossy = |s: &OutStream| matches!(&s.encode, Encode::Audio { codec, .. } if codec != "flac");
    let bitrate_of = |s: &OutStream| match &s.encode {
        Encode::Audio { bitrate, .. } => Some(bitrate.clone()),
        Encode::Copy => None,
    };
    let any_lossy = streams.iter().any(|s| lossy(s));
    let considered: Vec<&OutStream> = streams
        .iter()
        .map(|s| &**s)
        .filter(|s| !any_lossy || lossy(s))
        .collect();
    let current = common(considered.iter().map(|s| bitrate_of(s))).flatten();
    let text = match &current {
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
            .width(96.0)
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

/// The channel-layout combo over converting `streams`: their shared layout, or
/// "mixed". A pick applies to all of them.
fn channels_combo(ui: &mut egui::Ui, streams: &mut [&mut OutStream]) {
    let current = common(streams.iter().map(|s| match &s.encode {
        Encode::Audio { channels, .. } => *channels,
        Encode::Copy => None,
    }));
    let text = match current {
        Some(c) => CHANNELS
            .iter()
            .find(|(v, _)| *v == c)
            .map(|(_, label)| *label)
            .unwrap_or("source"),
        None => "mixed",
    };
    egui::ComboBox::from_id_salt("channels")
        .selected_text(text)
        .width(96.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_reports_shared_value_or_none() {
        assert_eq!(common([1, 1, 1].into_iter()), Some(1));
        assert_eq!(common([1, 2].into_iter()), None);
        assert_eq!(common(std::iter::empty::<i32>()), None);
    }
}
