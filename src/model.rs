//! The core data model. The whole app is a state-model editor over one
//! structure: a `Vec<OutStream>` in output order (see `Project`). Reorder =
//! reorder the vec; remove = drop from it; extract = a subset project with a
//! different output path; insert = add an input + streams referencing it.
//!
//! This module is deliberately pure data + tiny helpers. Turning a `Project`
//! into an ffmpeg command line lives in `args.rs`; building one from a probed
//! file lives in `probe.rs`.

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
}

/// How to encode a stream. `Copy` is the lossless fast path for everything
/// except audio conversion — five of the six core operations use it.
#[derive(Debug, Clone)]
pub enum Encode {
    Copy,
    /// The audio-conversion (re-encode) path. Constructed by the convert action
    /// in the UI (M4); until then it's exercised only by tests.
    #[allow(dead_code)]
    Audio {
        codec: String,
        bitrate_kbps: Option<u32>,
        channels: Option<u32>,
    },
}

/// User-editable tags and flags for a stream.
///
/// `language`/`title` are `Option`: `None` means "emit nothing", which lets the
/// original tag pass through the copy unchanged. (Actively *clearing* an
/// existing tag needs a different representation; that's an editing-milestone
/// concern, noted in the plan.)
#[derive(Debug, Clone, Default)]
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
}

/// The whole editing session: the inputs, the ordered output streams, and where
/// to write.
#[derive(Debug, Clone)]
pub struct Project {
    pub inputs: Vec<PathBuf>,
    pub streams: Vec<OutStream>,
    pub output: PathBuf,
    /// Duration of the primary input in seconds, from ffprobe `-show_format`.
    /// Kept here so the run layer can compute a progress percentage later (M2).
    /// `None` if ffprobe didn't report one.
    #[allow(dead_code)]
    pub duration_secs: Option<f64>,
}
