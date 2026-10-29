//! Preview clips: a short window of the real output, rendered to a temp file and
//! opened in the default player, so sync and subtitles can be checked without a
//! full export.
//!
//! The clip comes from the same model as the real run (`Project::preview_args`),
//! so offsets, mapping and conversions are exactly what Run would produce. Two
//! things differ on purpose:
//! - The output is always a temp `.mkv`: Matroska takes every codec, so a preview
//!   never fails on a container mismatch the validation hints already flag.
//! - The stream under test is the only stream of its kind in the clip, so the
//!   player can't open on another track (see `focused`).
//! - Video is re-encoded (fast, ≤ 720p) so it starts exactly at the start time
//!   rather than at the next keyframe (`args::PREVIEW_VIDEO`).
//!
//! Each clip gets a fresh file name, because a player can hold the previous one
//! open (and locked) while the next renders. Leftovers are swept at startup.

use crate::model::{Kind, Project};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// File-name prefix for preview clips in the temp dir; the startup sweep keys on it.
const PREFIX: &str = "interlace-preview-";

/// The clip lengths offered in the UI, in seconds.
pub const LENGTHS: [f64; 3] = [10.0, 30.0, 60.0];

/// A fresh temp path for the next clip: process id plus a per-process counter,
/// so neither a clip still open in a player nor another instance collides.
pub fn next_path() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{PREFIX}{}-{n}.mkv", std::process::id()))
}

/// Delete leftover clips from earlier sessions. Best-effort: a clip still open
/// in a player stays locked on Windows and is simply skipped.
pub fn sweep_temp() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(PREFIX) && name.ends_with(".mkv") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The project to preview: a copy of `project` where the stream under test is
/// the *only* stream of its kind (the others are left unmapped) and is marked
/// default. Players pick tracks by their own rules — the first track, or a
/// preferred language (MPC-HC) that an untagged added `.srt` never matches — so
/// leaving the player no other choice is the only reliable way to show it.
///
/// The stream under test is `selected` if it's a live audio/subtitle stream;
/// otherwise the first such stream with a sync offset, since
/// checking that offset is what a preview is usually for.
pub fn focused(project: &Project, selected: Option<usize>) -> Project {
    let mut clip = project.clone();
    let Some(focus) = focus(project, selected) else { return clip };

    let kind = clip.streams[focus].source.kind;
    for (i, s) in clip.streams.iter_mut().enumerate() {
        if s.source.kind == kind {
            s.removed = i != focus; // unmapped by `to_args`
            s.meta.default = i == focus;
        }
    }
    clip
}

/// The stream a preview tests: `selected` if it's a live audio/subtitle stream,
/// otherwise the first such stream with a sync offset. `None` previews every
/// track as it is. Shared by `focused` and the run card's "Testing …" line.
pub fn focus(project: &Project, selected: Option<usize>) -> Option<usize> {
    let testable = |i: usize| {
        let s = &project.streams[i];
        !s.removed && matches!(s.source.kind, Kind::Audio | Kind::Subtitle)
    };
    selected
        .filter(|&i| i < project.streams.len() && testable(i))
        .or_else(|| (0..project.streams.len()).find(|&i| testable(i) && project.streams[i].has_offset()))
}

/// Parse a start time typed by the user: `ss`, `mm:ss` or `hh:mm:ss`, each with
/// optional fractional seconds (`.` or `,`). `None` for anything else, including
/// negative values.
pub fn parse_timestamp(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut secs = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        let v: f64 = part.parse().ok()?;
        // Only the last field may be fractional, and every field after the first
        // must be below 60 (a leading `90:00` is fine).
        if v < 0.0 || !v.is_finite() || (!last && v.fract() != 0.0) || (i > 0 && v >= 60.0) {
            return None;
        }
        secs = secs * 60.0 + v;
    }
    Some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Encode, Input, Meta, OutStream, Source};

    #[test]
    fn parses_timestamp_forms() {
        assert_eq!(parse_timestamp("75"), Some(75.0));
        assert_eq!(parse_timestamp("1:15"), Some(75.0));
        assert_eq!(parse_timestamp("01:02:03.5"), Some(3723.5));
        assert_eq!(parse_timestamp(" 00:00:01,250 "), Some(1.25));
        assert_eq!(parse_timestamp("90:00"), Some(5400.0));
    }

    #[test]
    fn rejects_bad_timestamps() {
        for bad in ["", "abc", "1:60", "-5", "1.5:00", "1:2:3:4", "1::2"] {
            assert_eq!(parse_timestamp(bad), None, "{bad}");
        }
    }

    fn stream(index: usize, kind: Kind, default: bool) -> OutStream {
        OutStream::new(
            Source { input: 0, index, kind, codec: "x".into(), bitrate_kbps: None },
            Meta { default, ..Default::default() },
            Encode::Copy,
        )
    }

    fn project() -> Project {
        Project {
            inputs: vec![Input::new(PathBuf::from("in.mkv"))],
            streams: vec![
                stream(0, Kind::Video, true),
                stream(1, Kind::Audio, true),
                stream(2, Kind::Audio, false),
                stream(3, Kind::Subtitle, false),
            ],
            title: None,
            output: PathBuf::from("out.mkv"),
            duration_secs: None,
        }
    }

    /// `(source index, default)` of each stream that will be mapped, in order.
    fn layout(p: &Project) -> Vec<(usize, bool)> {
        p.streams
            .iter()
            .filter(|s| !s.removed)
            .map(|s| (s.source.index, s.meta.default))
            .collect()
    }

    #[test]
    fn focused_keeps_only_the_stream_under_test_of_its_kind() {
        let p = focused(&project(), Some(2));
        assert_eq!(layout(&p), [(0, true), (2, true), (3, false)]);
    }

    #[test]
    fn focused_leaves_video_and_no_focus_alone() {
        let original = layout(&project());
        for focus in [None, Some(0), Some(99)] {
            assert_eq!(layout(&focused(&project(), focus)), original, "{focus:?}");
        }
    }

    /// With nothing testable selected, an offset input's stream is the one under
    /// test: an embedded `.srt` replaces the file's own subtitle in the clip.
    #[test]
    fn focused_falls_back_to_a_stream_with_an_offset() {
        let mut p = project();
        p.inputs.push(Input::new(PathBuf::from("subs.srt")));
        let mut sub = stream(0, Kind::Subtitle, false);
        sub.source.input = 1;
        sub.offset_secs = 2.0;
        p.streams.push(sub);

        let clip = focused(&p, Some(0)); // video selected: not testable
        let subs: Vec<(usize, bool)> = clip
            .streams
            .iter()
            .filter(|s| s.source.kind == Kind::Subtitle && !s.removed)
            .map(|s| (s.source.input, s.meta.default))
            .collect();
        assert_eq!(subs, [(1, true)]);
    }
}
