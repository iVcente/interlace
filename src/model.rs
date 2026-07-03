//! The core data model. The whole app is a state-model editor over one
//! structure: a `Vec<OutStream>` in output order (see `Project`). Reorder =
//! reorder the vec; remove = drop from it; extract = a subset project with a
//! different output path; insert = add an input + streams referencing it.
//!
//! This module is deliberately pure data + tiny helpers. Turning a `Project`
//! into an ffmpeg command line lives in `args.rs`; building one from a probed
//! file lives in `probe.rs`.

use std::collections::HashSet;
use std::path::PathBuf;

/// The stream types we care about. The letter each maps to is the stream
/// specifier ffmpeg uses (`0:a:1`, `-c:a:0`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
    Subtitle,
    Attachment,
    Data,
}

impl Kind {
    /// The letter ffmpeg uses in stream specifiers: `0:a:1`, `-c:a:0`, ...
    pub fn spec(self) -> char {
        match self {
            Kind::Video => 'v',
            Kind::Audio => 'a',
            Kind::Subtitle => 's',
            Kind::Attachment => 't',
            Kind::Data => 'd',
        }
    }

    /// Map ffprobe's `codec_type` string onto a `Kind`. Types we don't model
    /// (there shouldn't be any for the containers we target) return `None` so
    /// the ingest layer can skip them rather than failing the whole probe.
    pub fn from_codec_type(s: &str) -> Option<Kind> {
        Some(match s {
            "video" => Kind::Video,
            "audio" => Kind::Audio,
            "subtitle" => Kind::Subtitle,
            "attachment" => Kind::Attachment,
            "data" => Kind::Data,
            _ => return None,
        })
    }
}

/// One `-i` input file, plus a timestamp shift for syncing it against the rest.
///
/// The primary input (index 0) always has `offset_secs == 0.0`. An embedded track
/// added in the editor can carry a nonzero offset, emitted as `-itsoffset` before
/// its `-i` (see `args.rs`) to nudge it earlier or later.
#[derive(Debug, Clone)]
pub struct Input {
    pub path: PathBuf,
    /// Timestamp shift in seconds applied via `-itsoffset` (positive delays this
    /// input, negative advances it). Always 0.0 for the primary input.
    pub offset_secs: f64,
}

impl Input {
    /// A plain input with no timestamp shift.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset_secs: 0.0,
        }
    }
}

/// Where an output stream's packets come from.
#[derive(Debug, Clone)]
pub struct Source {
    /// Which `-i` input this comes from (0-based).
    pub input: usize,
    /// Absolute stream index within that input, as reported by ffprobe.
    pub index: usize,
    pub kind: Kind,
    /// Display only, e.g. "flac". Surfaced in the stream table (M3).
    #[allow(dead_code)]
    pub codec: String,
    /// The source stream's measured bitrate in kbps, if ffprobe reported one.
    /// Used by `Bitrate::Auto` to follow the source; `None` when unknown.
    pub bitrate_kbps: Option<u32>,
}

/// The standard audio bitrate ladder (kbps), ascending. Shared by the inspector's
/// selectable list and `Bitrate::Auto`'s ceil-to-source logic.
pub const BITRATE_LADDER: [u32; 6] = [96, 128, 160, 192, 256, 320];

/// The target bitrate for an audio conversion.
#[derive(Debug, Clone, PartialEq)]
pub enum Bitrate {
    /// Follow the source: ceil its bitrate up to the next `BITRATE_LADDER` rung
    /// (saturating at the top), never downgrading. Falls back to `Default` when
    /// the source bitrate is unknown.
    Auto,
    /// Let the encoder pick — emit no `-b` flag.
    Default,
    /// An explicit target in kbps.
    Fixed(u32),
}

impl Bitrate {
    /// The concrete kbps target to emit, or `None` to omit `-b` entirely (encoder
    /// default). `source_kbps` is the probed source bitrate, if known.
    pub fn resolve(&self, source_kbps: Option<u32>) -> Option<u32> {
        match self {
            Bitrate::Default => None,
            Bitrate::Fixed(b) => Some(*b),
            Bitrate::Auto => source_kbps.map(ceil_to_ladder),
        }
    }
}

/// Smallest `BITRATE_LADDER` rung `>= kbps`, saturating at the top rung.
fn ceil_to_ladder(kbps: u32) -> u32 {
    BITRATE_LADDER
        .iter()
        .copied()
        .find(|&r| r >= kbps)
        .unwrap_or(*BITRATE_LADDER.last().unwrap())
}

/// How to encode a stream. `Copy` is the lossless fast path for everything
/// except audio conversion — five of the six core operations use it.
#[derive(Debug, Clone)]
pub enum Encode {
    Copy,
    /// The audio-conversion (re-encode) path, set by the inspector's conversion
    /// controls.
    Audio {
        codec: String,
        bitrate: Bitrate,
        channels: Option<u32>,
    },
}

impl Encode {
    /// The starting point when the user converts a stream: 192 kbps AAC, keeping
    /// the source channel layout. Codec/bitrate/channels are then tunable.
    pub fn default_audio() -> Self {
        Encode::Audio {
            codec: "aac".into(),
            bitrate: Bitrate::Fixed(192),
            channels: None,
        }
    }
}

/// User-editable tags and flags for a stream.
///
/// `language`/`title` are `Option`: `None` means "emit nothing", which lets the
/// original tag pass through the copy unchanged. (Actively *clearing* an
/// existing tag needs a different representation; that's an editing-milestone
/// concern, noted in the plan.)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    /// ISO 639-2, e.g. "jpn".
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: bool,
    pub forced: bool,
}

/// One output stream. Its position in `Project::streams` *is* its output order.
#[derive(Debug, Clone)]
pub struct OutStream {
    pub source: Source,
    pub meta: Meta,
    pub encode: Encode,
    /// Pending-removal flag. A removed stream stays in the table (dimmed, badged
    /// "remove") so the change is visible and reversible, but `to_args()` skips
    /// it — see `Project::to_args`.
    pub removed: bool,
    /// The metadata as first probed, kept as the baseline the inspector edits are
    /// diffed against for the table's change badges. Original `encode` is always
    /// `Copy` at load, so "converted" is derived from `encode` and not snapshotted.
    pub orig_meta: Meta,
    /// Whether this stream was synthesized in the editor (e.g. a converted copy
    /// added alongside its source) rather than probed from the file. Added
    /// streams aren't part of the original, so the UI deletes them outright
    /// instead of offering soft-removal / revert / extract.
    pub added: bool,
}

impl OutStream {
    /// Build a stream, snapshotting `meta` as the change-diff baseline and
    /// starting un-removed. The single place `orig_meta`/`removed`/`added` are
    /// seeded; callers that synthesize a stream set `added` afterward.
    pub fn new(source: Source, meta: Meta, encode: Encode) -> Self {
        Self {
            source,
            orig_meta: meta.clone(),
            meta,
            encode,
            removed: false,
            added: false,
        }
    }

    /// Language or title edited away from the probed original.
    pub fn tags_changed(&self) -> bool {
        self.meta.language != self.orig_meta.language || self.meta.title != self.orig_meta.title
    }

    /// Default/forced disposition edited away from the probed original.
    pub fn flags_changed(&self) -> bool {
        self.meta.default != self.orig_meta.default || self.meta.forced != self.orig_meta.forced
    }

    /// This stream re-encodes rather than stream-copies (audio-convert path).
    pub fn converted(&self) -> bool {
        !matches!(self.encode, Encode::Copy)
    }
}

/// The whole editing session: the inputs, the ordered output streams, and where
/// to write.
#[derive(Debug, Clone)]
pub struct Project {
    pub inputs: Vec<Input>,
    pub streams: Vec<OutStream>,
    /// Container-level title tag for the whole output file. `None` emits nothing
    /// (the source container title, if any, passes through the copy); `Some`
    /// sets `-metadata title=…`. Distinct from per-stream `Meta::title`.
    pub title: Option<String>,
    pub output: PathBuf,
    /// Duration of the primary input in seconds, from ffprobe `-show_format`.
    /// Kept here so the run layer can compute a progress percentage later (M2).
    /// `None` if ffprobe didn't report one.
    #[allow(dead_code)]
    pub duration_secs: Option<f64>,
}

impl Project {
    /// Drop any non-primary input no longer referenced by *any* stream, reindexing
    /// the `source.input` of the survivors. Called after a hard delete so that
    /// removing the last stream of an embedded track also drops its `-i`.
    ///
    /// Membership is by vec presence, not liveness: a soft-`removed` stream still
    /// pins its input (Restore must keep working), so only hard-deleted streams can
    /// orphan one. Indices are walked high→low so each removal only shifts indices
    /// we've already visited. Input 0 (the primary) is never pruned.
    pub(crate) fn prune_orphan_inputs(&mut self) {
        let used: HashSet<usize> = self.streams.iter().map(|s| s.source.input).collect();
        for k in (1..self.inputs.len()).rev() {
            if !used.contains(&k) {
                self.inputs.remove(k);
                for s in &mut self.streams {
                    if s.source.input > k {
                        s.source.input -= 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(input: usize) -> OutStream {
        OutStream::new(
            Source {
                input,
                index: 0,
                kind: Kind::Audio,
                codec: "aac".into(),
                bitrate_kbps: None,
            },
            Meta::default(),
            Encode::Copy,
        )
    }

    fn project(inputs: usize, streams: Vec<OutStream>) -> Project {
        Project {
            inputs: (0..inputs)
                .map(|i| Input::new(PathBuf::from(format!("in{i}.mkv"))))
                .collect(),
            streams,
            title: None,
            output: PathBuf::from("out.mkv"),
            duration_secs: None,
        }
    }

    #[test]
    fn prune_orphan_inputs_reindexes() {
        // 3 inputs; only inputs 0 and 2 are still referenced by a stream.
        let mut p = project(3, vec![stream(0), stream(2)]);
        p.prune_orphan_inputs();

        // Input 1 (orphaned) is dropped; the input-2 stream reindexes down to 1.
        assert_eq!(p.inputs.len(), 2);
        assert_eq!(p.inputs[1].path, PathBuf::from("in2.mkv"));
        assert_eq!(p.streams[1].source.input, 1);
    }

    #[test]
    fn prune_keeps_input_zero() {
        // Even with no streams at all, the primary input is never pruned.
        let mut p = project(1, vec![]);
        p.prune_orphan_inputs();
        assert_eq!(p.inputs.len(), 1);
    }

    #[test]
    fn prune_by_membership_not_liveness() {
        // A soft-removed stream still pins its input — Restore must keep working.
        let mut removed = stream(1);
        removed.removed = true;
        let mut p = project(2, vec![stream(0), removed]);
        p.prune_orphan_inputs();
        assert_eq!(p.inputs.len(), 2, "soft-removed stream must keep its input");
    }

    #[test]
    fn auto_bitrate_ceils_to_ladder_never_downgrading() {
        // On-rung stays put; between-rungs rounds up; above the top saturates.
        assert_eq!(Bitrate::Auto.resolve(Some(96)), Some(96));
        assert_eq!(Bitrate::Auto.resolve(Some(128)), Some(128));
        assert_eq!(Bitrate::Auto.resolve(Some(137)), Some(160));
        assert_eq!(Bitrate::Auto.resolve(Some(192)), Some(192));
        assert_eq!(Bitrate::Auto.resolve(Some(210)), Some(256));
        assert_eq!(Bitrate::Auto.resolve(Some(900)), Some(320));
    }

    #[test]
    fn auto_bitrate_without_source_falls_back_to_default() {
        assert_eq!(Bitrate::Auto.resolve(None), None);
    }

    #[test]
    fn default_and_fixed_bitrate_ignore_source() {
        assert_eq!(Bitrate::Default.resolve(Some(256)), None);
        assert_eq!(Bitrate::Fixed(192).resolve(Some(256)), Some(192));
        assert_eq!(Bitrate::Fixed(192).resolve(None), Some(192));
    }
}
