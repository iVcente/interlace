//! Preview clips: a short window of the real output, rendered to a temp file and
//! opened in the default player, so sync and subtitles can be checked without a
//! full export.
//!
//! The clip comes from the same model as the real run (`Project::preview_args`),
//! so offsets, mapping and conversions are exactly what Run would produce. Two
//! things differ on purpose:
//! - The output is always a temp `.mkv`: Matroska takes every codec, so a preview
//!   never fails on a container mismatch the validation hints already flag.
//! - Audio and subtitles are each narrowed to the track under test, so the player
//!   can't open on another one (see `focused`). The two kinds are chosen
//!   independently: one track, or left out.
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

/// What a preview does with one kind of track.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackPick {
    /// Only this track. Named by input path and source index because rows
    /// move; `copy` tells apart converted copies of one source stream.
    Track { input: PathBuf, index: usize, copy: usize },
    /// Leave this kind out of the clip.
    Off,
}

impl TrackPick {
    /// A pin on `streams[row]`.
    pub fn track(project: &Project, row: usize) -> Self {
        let source = &project.streams[row].source;
        let input = &project.inputs[source.input].path;
        let copy = (0..row).filter(|&i| is_stream(project, i, input, source.index)).count();
        Self::Track { input: input.clone(), index: source.index, copy }
    }

    /// The live row this pins, if it pins one and that row is still live.
    pub fn row(&self, project: &Project) -> Option<usize> {
        let Self::Track { input, index, copy } = self else { return None };
        (0..project.streams.len())
            .filter(|&i| is_stream(project, i, input, *index))
            .nth(*copy)
            .filter(|&i| !project.streams[i].removed)
    }
}

/// Whether `streams[row]` comes from source stream `index` of the input at `input`.
fn is_stream(project: &Project, row: usize, input: &std::path::Path, index: usize) -> bool {
    let source = &project.streams[row].source;
    source.index == index && project.inputs[source.input].path == input
}

/// The pick a kind starts from, and returns to when its track goes away: what a
/// player would open on, so the clip matches the output. That's the live track
/// flagged default; failing that, the first audio track, or no subtitles.
fn default_pick(project: &Project, kind: Kind) -> TrackPick {
    let live = |i: &usize| project.streams[*i].source.kind == kind && !project.streams[*i].removed;
    let mut rows = (0..project.streams.len()).filter(live);
    let row = rows
        .clone()
        .find(|&i| project.streams[i].meta.default)
        .or_else(|| if kind == Kind::Audio { rows.next() } else { None });
    row.map_or(TrackPick::Off, |row| TrackPick::track(project, row))
}

/// The preview's choice for each kind of track it narrows.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackPicks {
    pub audio: TrackPick,
    pub subtitle: TrackPick,
}

impl TrackPicks {
    /// The picks a freshly opened `project` starts from.
    pub fn initial(project: &Project) -> Self {
        Self { audio: default_pick(project, Kind::Audio), subtitle: default_pick(project, Kind::Subtitle) }
    }

    /// Point `streams[row]`'s kind at it. Video has no pick, so it changes nothing.
    pub fn select(&mut self, project: &Project, row: usize) {
        let pick = TrackPick::track(project, row);
        match project.streams[row].source.kind {
            Kind::Audio => self.audio = pick,
            Kind::Subtitle => self.subtitle = pick,
            _ => {}
        }
    }

    /// Reset any pick whose track was deleted or marked for removal.
    pub fn prune(&mut self, project: &Project) {
        for (kind, pick) in [(Kind::Audio, &mut self.audio), (Kind::Subtitle, &mut self.subtitle)] {
            if matches!(pick, TrackPick::Track { .. }) && pick.row(project).is_none() {
                *pick = default_pick(project, kind);
            }
        }
    }
}

/// The project to preview: a copy of `project` where the picked track is the
/// *only* one of its kind (the others are left unmapped) and marked default.
/// Players pick tracks by their own rules — the first track, or a preferred
/// language (MPC-HC) that an untagged added `.srt` never matches — so leaving the
/// player no other choice is the only reliable way to show it.
pub fn focused(project: &Project, picks: &TrackPicks) -> Project {
    let mut clip = project.clone();
    for (kind, pick) in [(Kind::Audio, &picks.audio), (Kind::Subtitle, &picks.subtitle)] {
        let row = pick.row(project);
        for (i, s) in clip.streams.iter_mut().enumerate().filter(|(_, s)| s.source.kind == kind) {
            s.removed = row != Some(i); // unmapped by `to_args`
            s.meta.default = row == Some(i);
        }
    }
    clip
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

    /// The picked tracks are the only ones of their kinds, each marked default.
    #[test]
    fn focused_keeps_only_the_picked_tracks() {
        let p = project();
        let picks = TrackPicks { audio: TrackPick::track(&p, 2), subtitle: TrackPick::track(&p, 3) };
        assert_eq!(layout(&focused(&p, &picks)), [(0, true), (2, true), (3, true)]);
    }

    #[test]
    fn focused_leaves_out_a_kind_set_to_off() {
        let p = project();
        let picks = TrackPicks { audio: TrackPick::track(&p, 1), subtitle: TrackPick::Off };
        assert_eq!(layout(&focused(&p, &picks)), [(0, true), (1, true)]);
    }

    /// A new project starts from the tracks a player would open on: the flagged
    /// audio, or else the first; a subtitle only if one is flagged.
    #[test]
    fn initial_picks_use_the_default_flags() {
        let mut p = project();
        let picks = TrackPicks::initial(&p);
        assert_eq!(picks.audio, TrackPick::track(&p, 1));
        assert_eq!(picks.subtitle, TrackPick::Off);

        p.streams[1].meta.default = false;
        p.streams[2].meta.default = true;
        p.streams[3].meta.default = true;
        let picks = TrackPicks::initial(&p);
        assert_eq!(picks.audio, TrackPick::track(&p, 2));
        assert_eq!(picks.subtitle, TrackPick::track(&p, 3));

        p.streams[2].meta.default = false;
        assert_eq!(TrackPicks::initial(&p).audio, TrackPick::track(&p, 1), "first audio");
    }

    /// Selecting sets only the stream's own kind, and video sets nothing.
    #[test]
    fn select_sets_only_its_own_kind() {
        let p = project();
        let mut picks = TrackPicks::initial(&p);
        picks.select(&p, 0);
        assert_eq!(picks, TrackPicks::initial(&p));
        picks.select(&p, 3);
        assert_eq!(picks.subtitle, TrackPick::track(&p, 3));
        assert_eq!(picks.audio, TrackPick::track(&p, 1));
    }

    /// A pick follows its track when rows move, and tells converted copies of one
    /// source stream apart.
    #[test]
    fn a_pick_names_its_track_across_reorders_and_copies() {
        let mut p = project();
        let pick = TrackPick::track(&p, 2);
        p.streams.swap(1, 2);
        assert_eq!(pick.row(&p), Some(1));

        p.streams.push(stream(2, Kind::Audio, false));
        let copy = TrackPick::track(&p, 4);
        assert_eq!(copy.row(&p), Some(4));
        assert_eq!(pick.row(&p), Some(1));
    }

    /// A pick on a track marked for removal resets, and None stays None.
    #[test]
    fn prune_resets_a_dead_pick() {
        let mut p = project();
        let mut picks = TrackPicks { audio: TrackPick::track(&p, 2), subtitle: TrackPick::Off };
        picks.prune(&p);
        assert_eq!(picks.audio, TrackPick::track(&p, 2));

        p.streams[2].removed = true;
        picks.prune(&p);
        assert_eq!(picks.audio, TrackPick::track(&p, 1));
        assert_eq!(picks.subtitle, TrackPick::Off);
    }
}
