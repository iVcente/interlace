//! Serializing a `Project` into ffmpeg arguments.
//!
//! The single most error-prone rule ffmpeg imposes: **output stream indices are
//! assigned per type, in `-map` order.** The 2nd audio stream mapped becomes
//! `:a:1` for every later option, regardless of its source index. So we walk the
//! streams in output order and keep a per-type counter; every `-c` / `-metadata`
//! / `-disposition` uses that output index, not the source index.
//!
//! `preview_args` reuses the same body to render a short window of the output
//! for a preview clip; its doc explains how seeks and sync offsets combine.

use crate::model::{Encode, Kind, OutStream, Project};
use std::collections::HashMap;
use std::path::Path;

/// Whether the output container accepts per-stream metadata and disposition.
/// Raw/elementary muxers (`.aac`, `.ac3`, `.srt`, a raw `.h264` bitstream, …)
/// reject `-metadata:s`/`-disposition` — extraction targets one of these — so we
/// emit only `-map` + `-c copy` for them. Real containers (mkv/mp4/mov/webm, and
/// the mka/mks single-stream fallbacks) take the tags.
fn output_takes_stream_tags(output: &Path) -> bool {
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    !matches!(
        ext.as_deref(),
        Some(
            "aac" | "ac3" | "eac3" | "dts" | "mp3" | "flac" | "opus" | "ogg" | "wav" | "thd"
                | "h264" | "264" | "h265" | "hevc" | "265" | "av1" | "obu" | "srt" | "ass"
                | "ssa" | "vtt" | "sup" | "sub"
        )
    )
}

/// The ffmpeg encoder to name in `-c` for a conversion target. The model keeps
/// the *codec* name (`opus`) because validation compares it against ffprobe's
/// `codec_name`; only the argv needs the encoder. `-c opus` would pick ffmpeg's
/// native Opus encoder, which is experimental and refuses to run without
/// `-strict -2`, so we name `libopus` explicitly. The other targets either are
/// the native encoder's name (`aac`, `ac3`, `flac`) or resolve to the right
/// default encoder by codec (`mp3` → `libmp3lame`).
fn encoder_for(codec: &str) -> &str {
    match codec {
        "opus" => "libopus",
        other => other,
    }
}

/// How a preview clip encodes video, as `(option, value)` pairs that get the
/// stream's output specifier appended (`-c:v:0`, ...). Stream-copied video can
/// only start on a keyframe, often seconds into the window, and players freeze
/// on the first frame until it arrives. Re-encoding lets the input seek trim to
/// the exact start. It's a throwaway clip, so speed wins: `ultrafast`, scaled
/// down to at most 720p, and 8-bit 4:2:0 so any player can decode it.
const PREVIEW_VIDEO: &[(&str, &str)] = &[
    ("-c", "libx264"),
    ("-preset", "ultrafast"),
    ("-crf", "23"),
    ("-pix_fmt", "yuv420p"),
    // `\,` escapes the comma from ffmpeg's filtergraph parser; no shell involved.
    ("-filter", r"scale=-2:min(ih\,720)"),
];

/// Emit a `-metadata:s:<spec> key=value` pair for a per-stream tag, matching the
/// output to the edited value:
/// - `Some(v)` → set the tag to `v`, overriding the source's passthrough value.
/// - `None` but the source *had* one (`orig` is `Some`) → emit an empty `key=`,
///   which ffmpeg treats as a deletion. Without this the original tag would copy
///   straight through, so an emptied field would appear to do nothing.
/// - `None` and the source had none → emit nothing.
fn push_tag(a: &mut Vec<String>, spec: &str, key: &str, cur: &Option<String>, orig: &Option<String>) {
    match cur {
        Some(v) => {
            a.push(format!("-metadata:s:{spec}"));
            a.push(format!("{key}={v}"));
        }
        None if orig.is_some() => {
            a.push(format!("-metadata:s:{spec}"));
            a.push(format!("{key}="));
        }
        None => {}
    }
}

/// Which `-i`s the command opens, and which one each live stream is mapped from.
///
/// ffmpeg's `-itsoffset` shifts a whole input, so a stream whose offset differs
/// from the rest of its file is read from a second `-i` of the same file that
/// carries that offset. Every model input keeps its own `-i` at its own index
/// first — input 0 stays first, which is where ffmpeg takes chapters and global
/// metadata from — and the extra reads follow, one per distinct (file, offset).
struct InputPlan {
    /// `(model input index, offset in ms)` for each `-i`, in argv order.
    inputs: Vec<(usize, i64)>,
    /// For each live stream (in output order), the argv `-i` index it maps from.
    map_from: Vec<usize>,
}

impl InputPlan {
    /// Plan the inputs for `live`, the project's mapped streams in output order.
    fn new(project: &Project, live: &[&OutStream]) -> Self {
        // A model input's own `-i` takes the offset of its first live stream, so
        // a single shifted track (an added `.srt`) needs no second read. The
        // primary keeps 0: shifting it would shift chapters and every other track.
        let base = |k: usize| -> i64 {
            if k == 0 {
                return 0;
            }
            live.iter()
                .find(|s| s.source.input == k)
                .map_or(0, |s| s.offset_ms())
        };
        let mut inputs: Vec<(usize, i64)> = (0..project.inputs.len()).map(|k| (k, base(k))).collect();
        let map_from = live
            .iter()
            .map(|s| {
                let key = (s.source.input, s.offset_ms());
                if inputs[s.source.input] == key {
                    return s.source.input;
                }
                inputs.iter().position(|&i| i == key).unwrap_or_else(|| {
                    inputs.push(key);
                    inputs.len() - 1
                })
            })
            .collect();
        Self { inputs, map_from }
    }
}

impl Project {
    /// Render this project as the argument vector for `ffmpeg` (the program name
    /// itself is not included). Passed straight to `std::process::Command`, so no
    /// shell quoting is needed or wanted.
    ///
    /// Deliberately does **not** emit `-y`/`-n`: overwrite policy is decided by
    /// the run layer after an existence check, not baked into the command here.
    pub fn to_args(&self) -> Vec<String> {
        self.to_args_windowed(None)
    }

    /// The argv for a short preview clip: this project's exact command, cut to
    /// `len` seconds from `start` (output time) and written to `out`.
    ///
    /// Each input is seeked on its own with `-ss` before its `-i`. ffmpeg shifts
    /// an input's timestamps by `itsoffset - seek`, so an input with offset `o` is
    /// seeked to `start - o` *instead of* taking `-itsoffset`: a source time `t`
    /// lands at `t + o - start`, the same relative sync as the full run, and the
    /// cues an offset moves into the window are read rather than skipped. Near the
    /// file start the seek clamps at 0 and the remainder goes back into
    /// `-itsoffset` (see `to_args_windowed`).
    ///
    /// An input seek is only coarse under stream copy: video restarts at the
    /// keyframe before the seek point, and text subtitle demuxers (`.srt`) don't
    /// seek at all, so every earlier cue arrives with a negative timestamp. The
    /// muxer would then shift the whole clip to make those non-negative, leaving
    /// the window minutes in. An *output* `-ss 0` drops everything before the
    /// window instead, so the clip's time 0 is `start`. Video is re-encoded
    /// (`PREVIEW_VIDEO`) so it, too, starts exactly there rather than at the next
    /// keyframe.
    pub fn preview_args(&self, start: f64, len: f64, out: &Path) -> Vec<String> {
        let mut clip = self.clone();
        clip.output = out.to_path_buf();
        clip.to_args_windowed(Some((start, len)))
    }

    /// The shared body of [`Project::to_args`] and [`Project::preview_args`].
    /// `window` is `(start, len)` in output seconds; `None` renders everything.
    fn to_args_windowed(&self, window: Option<(f64, f64)>) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();

        // Only streams not marked for removal reach the output; a removed stream
        // is simply never mapped (and doesn't count toward per-type renumbering).
        let live: Vec<&OutStream> = self.streams.iter().filter(|s| !s.removed).collect();
        let plan = InputPlan::new(self, &live);

        // Inputs, in plan order — their position defines the `-i` index the maps
        // refer to. A shifted one gets `-itsoffset <secs>` right before its `-i`.
        // Offsets are integer milliseconds so tiny float noise never emits a
        // spurious one, and format with a fixed `.` decimal (locale-independent,
        // no `1e-1` forms).
        for &(input, offset_ms) in &plan.inputs {
            let offset = offset_ms as f64 / 1000.0;
            // A preview seeks this input to `start - offset`, clamped at 0; only
            // the part the clamp couldn't absorb is left for `-itsoffset`.
            let (seek, shift) = match window {
                Some((start, _)) => {
                    let seek = (start - offset).max(0.0);
                    (seek, offset - start + seek)
                }
                None => (0.0, offset),
            };
            if seek >= 0.0005 {
                a.push("-ss".into());
                a.push(format!("{seek:.3}"));
            }
            let ms = (shift * 1000.0).round() as i64;
            if ms != 0 {
                a.push("-itsoffset".into());
                a.push(format!("{:.3}", ms as f64 / 1000.0)); // e.g. "0.200", "-0.150"
            }
            a.push("-i".into());
            a.push(self.inputs[input].path.display().to_string());
        }

        // Maps first, in output order. This is what actually fixes ordering:
        // ffmpeg emits streams in the order they're mapped.
        for (s, arg_input) in live.iter().zip(&plan.map_from) {
            a.push("-map".into());
            a.push(format!("{arg_input}:{}", s.source.index));
        }

        // Raw/elementary outputs (extraction) reject per-stream tags entirely.
        let emit_tags = output_takes_stream_tags(&self.output);

        // Container-level title, for real containers only. Emitted once, up front,
        // and independent of per-stream metadata (that's `-metadata:s`, below).
        if emit_tags && let Some(title) = &self.title {
            a.push("-metadata".into());
            a.push(format!("title={title}"));
        }

        // Per-type output counter: the Nth audio we mapped is `:a:<n>`. Claim this
        // stream's index and bump the counter up front, so the two early-outs below
        // (attachment/data, raw output) can't forget to advance it.
        let mut counter: HashMap<char, usize> = HashMap::new();
        for s in &live {
            let c = s.source.kind.spec();
            let slot = counter.entry(c).or_insert(0);
            let n = *slot;
            *slot += 1;
            let spec = format!("{c}:{n}"); // e.g. "a:0"

            // Codec: copy for everything except the audio-conversion path — and,
            // in a preview, video (see `PREVIEW_VIDEO`).
            match &s.encode {
                Encode::Copy if window.is_some() && s.source.kind == Kind::Video => {
                    for (flag, value) in PREVIEW_VIDEO {
                        a.push(format!("{flag}:{spec}"));
                        a.push((*value).into());
                    }
                }
                Encode::Copy => {
                    a.push(format!("-c:{spec}"));
                    a.push("copy".into());
                }
                Encode::Audio {
                    codec,
                    bitrate,
                    channels,
                } => {
                    a.push(format!("-c:{spec}"));
                    a.push(encoder_for(codec).to_owned());
                    // `Auto` follows the probed source bitrate; `Default` resolves
                    // to `None` and emits no `-b` at all (encoder default).
                    if let Some(b) = bitrate.resolve(s.source.bitrate_kbps) {
                        a.push(format!("-b:{spec}"));
                        a.push(format!("{b}k"));
                    }
                    if let Some(ch) = channels {
                        a.push(format!("-ac:{spec}"));
                        a.push(ch.to_string());
                    }
                }
            }

            // Attachments (embedded fonts) and data streams are copied through,
            // but ffmpeg warns/errors on `-metadata`/`-disposition` for those
            // types — as do raw/elementary output muxers — so stop here for them.
            if matches!(s.source.kind, Kind::Attachment | Kind::Data) || !emit_tags {
                continue;
            }

            // Metadata. A set value overrides the source; emptying a tag the
            // source *had* emits an explicit clear (see `push_tag`).
            push_tag(&mut a, &spec, "language", &s.meta.language, &s.orig_meta.language);
            push_tag(&mut a, &spec, "title", &s.meta.title, &s.orig_meta.title);

            // Always emit disposition — even when empty (`0`) — so reordering can
            // never strand a stale `default`/`forced` flag on the wrong stream.
            let mut flags = Vec::new();
            if s.meta.default {
                flags.push("default");
            }
            if s.meta.forced {
                flags.push("forced");
            }
            a.push(format!("-disposition:{spec}"));
            a.push(if flags.is_empty() {
                "0".into()
            } else {
                flags.join("+")
            });
        }

        if let Some((_, len)) = window {
            // Output-side: drop whatever the coarse input seeks let through from
            // before the window (see `preview_args`).
            a.push("-ss".into());
            a.push("0".into());
            a.push("-t".into());
            a.push(format!("{len:.3}"));
        }
        a.push(self.output.display().to_string());
        a
    }
}

#[cfg(test)]
mod tests {
    use crate::model::*;
    use std::path::{Path, PathBuf};

    // --- small builders to keep the tests readable ---------------------------

    fn src(input: usize, index: usize, kind: Kind, codec: &str) -> Source {
        Source {
            input,
            index,
            kind,
            codec: codec.into(),
            bitrate_kbps: None,
        }
    }

    fn keep(source: Source, meta: Meta) -> OutStream {
        OutStream::new(source, meta, Encode::Copy)
    }

    fn lang(l: &str) -> Meta {
        Meta {
            language: Some(l.into()),
            ..Default::default()
        }
    }

    fn project(inputs: &[&str], streams: Vec<OutStream>) -> Project {
        Project {
            inputs: inputs.iter().map(|p| Input::new(PathBuf::from(p))).collect(),
            streams,
            title: None,
            output: PathBuf::from("out.mkv"),
            duration_secs: None,
        }
    }

    /// Every value that immediately follows an exact-match `flag` token.
    fn values_after(args: &[String], flag: &str) -> Vec<String> {
        args.iter()
            .zip(args.iter().skip(1))
            .filter(|(f, _)| f.as_str() == flag)
            .map(|(_, v)| v.clone())
            .collect()
    }

    // --- the five milestone-1 cases ------------------------------------------

    /// Reorder: `-map` order follows output order, AND per-type output indices
    /// renumber to match (moving jpn ahead of eng makes jpn `a:0`).
    #[test]
    fn reorder_renumbers_output_indices() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let eng = keep(src(0, 1, Kind::Audio, "aac"), lang("eng"));
        let jpn = keep(src(0, 2, Kind::Audio, "aac"), lang("jpn"));

        // Output order: video, jpn, eng (jpn dragged ahead of eng).
        let args = project(&["in.mkv"], vec![v, jpn, eng]).to_args();
        let joined = args.join(" ");

        assert_eq!(values_after(&args, "-map"), ["0:0", "0:2", "0:1"]);
        // jpn is now the first audio → a:0; eng is second → a:1.
        assert!(joined.contains("-metadata:s:a:0 language=jpn"), "{joined}");
        assert!(joined.contains("-metadata:s:a:1 language=eng"), "{joined}");
    }

    /// Remove via positive mapping: the dropped stream is simply absent, never
    /// negative-mapped, and survivors renumber.
    #[test]
    fn remove_uses_positive_mapping() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let jpn = keep(src(0, 2, Kind::Audio, "aac"), lang("jpn"));
        // eng (source 0:1) is dropped by never being included.

        let args = project(&["in.mkv"], vec![v, jpn]).to_args();
        let joined = args.join(" ");

        assert_eq!(values_after(&args, "-map"), ["0:0", "0:2"]);
        assert!(!joined.contains("-map -"), "no negative mapping: {joined}");
        assert!(!joined.contains("language=eng"), "dropped stream absent: {joined}");
        // jpn is the only audio now → a:0.
        assert!(joined.contains("-metadata:s:a:0 language=jpn"), "{joined}");
    }

    /// Soft removal: a stream flagged `removed` is never mapped and doesn't count
    /// toward per-type renumbering, exactly as if it had been dropped from the vec.
    #[test]
    fn removed_stream_excluded_from_output() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let mut eng = keep(src(0, 1, Kind::Audio, "aac"), lang("eng"));
        eng.removed = true; // pending removal
        let jpn = keep(src(0, 2, Kind::Audio, "aac"), lang("jpn"));

        let args = project(&["in.mkv"], vec![v, eng, jpn]).to_args();
        let joined = args.join(" ");

        assert_eq!(values_after(&args, "-map"), ["0:0", "0:2"]);
        assert!(!joined.contains("language=eng"), "removed stream absent: {joined}");
        // jpn is now the only audio → a:0.
        assert!(joined.contains("-metadata:s:a:0 language=jpn"), "{joined}");
    }

    /// Audio convert: the one re-encode path, on the correct output index.
    #[test]
    fn audio_convert_emits_codec_bitrate_channels() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let audio = OutStream::new(
            src(0, 1, Kind::Audio, "flac"),
            Meta::default(),
            Encode::Audio {
                codec: "aac".into(),
                bitrate: Bitrate::Fixed(192),
                channels: Some(2),
            },
        );

        let joined = project(&["in.mkv"], vec![v, audio]).to_args().join(" ");

        assert!(joined.contains("-c:v:0 copy"), "{joined}");
        assert!(
            joined.contains("-c:a:0 aac -b:a:0 192k -ac:a:0 2"),
            "{joined}"
        );
    }

    /// AUTO follows the probed source bitrate, ceiling it up to the ladder: a
    /// 137k source becomes a 160k target.
    #[test]
    fn auto_bitrate_follows_source_ceiled_to_ladder() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let mut source = src(0, 1, Kind::Audio, "aac");
        source.bitrate_kbps = Some(137);
        let audio = OutStream::new(
            source,
            Meta::default(),
            Encode::Audio {
                codec: "opus".into(),
                bitrate: Bitrate::Auto,
                channels: None,
            },
        );

        let joined = project(&["in.mkv"], vec![v, audio]).to_args().join(" ");
        assert!(joined.contains("-c:a:0 libopus -b:a:0 160k"), "{joined}");
    }

    /// AUTO with an unknown source bitrate, and DEFAULT, both emit no `-b` flag.
    #[test]
    fn auto_without_source_and_default_omit_bitrate() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let auto = OutStream::new(
            src(0, 1, Kind::Audio, "aac"), // bitrate_kbps: None
            Meta::default(),
            Encode::Audio {
                codec: "opus".into(),
                bitrate: Bitrate::Auto,
                channels: None,
            },
        );
        let default = OutStream::new(
            src(0, 2, Kind::Audio, "aac"),
            Meta::default(),
            Encode::Audio {
                codec: "opus".into(),
                bitrate: Bitrate::Default,
                channels: None,
            },
        );

        let args = project(&["in.mkv"], vec![v, auto, default]).to_args();
        assert!(values_after(&args, "-b:a:0").is_empty(), "{args:?}");
        assert!(values_after(&args, "-b:a:1").is_empty(), "{args:?}");
    }

    /// Insert from a second input: two `-i`, a stream referencing input 1, and
    /// per-type numbering that spans inputs (commentary is the 2nd audio → a:1).
    #[test]
    fn insert_from_second_input() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let eng = keep(src(0, 1, Kind::Audio, "aac"), lang("eng"));
        let commentary = OutStream::new(
            src(1, 0, Kind::Audio, "flac"),
            Meta {
                title: Some("Commentary".into()),
                ..Default::default()
            },
            Encode::Copy,
        );

        let args = project(&["vid.mkv", "new.flac"], vec![v, eng, commentary]).to_args();
        let joined = args.join(" ");

        assert_eq!(values_after(&args, "-i"), ["vid.mkv", "new.flac"]);
        assert_eq!(values_after(&args, "-map"), ["0:0", "0:1", "1:0"]);
        // eng is a:0, commentary (from the 2nd input) is a:1.
        assert!(joined.contains("-metadata:s:a:1 title=Commentary"), "{joined}");
    }

    /// Disposition-clearing on reorder: because disposition is always emitted,
    /// moving the default stream can't strand a stale flag. After reordering so a
    /// non-default stream sits at a:0, a:0 is explicitly cleared to `0` and the
    /// default flag rides along to a:1 with its `default+forced` join intact.
    #[test]
    fn disposition_cleared_on_reorder() {
        let default_forced = keep(
            src(0, 1, Kind::Audio, "aac"),
            Meta {
                default: true,
                forced: true,
                ..Default::default()
            },
        );
        let plain = keep(src(0, 2, Kind::Audio, "aac"), Meta::default());

        // Reordered so the plain stream is first (a:0) and the flagged one second.
        let joined = project(&["in.mkv"], vec![plain, default_forced])
            .to_args()
            .join(" ");

        assert!(joined.contains("-disposition:a:0 0"), "cleared at a:0: {joined}");
        assert!(
            joined.contains("-disposition:a:1 default+forced"),
            "flags ride along to a:1: {joined}"
        );
    }

    /// Emptying a tag the source *had* emits an explicit clear (`title=`) so the
    /// original doesn't pass through the copy; a tag set to a value overrides; a
    /// field that was empty to begin with emits nothing.
    #[test]
    fn emptied_tag_emits_explicit_clear() {
        // Probed with a title + language, then the user cleared both in the editor.
        let mut cleared = keep(
            src(0, 1, Kind::Audio, "aac"),
            Meta {
                title: Some("Commentary".into()),
                language: Some("eng".into()),
                ..Default::default()
            },
        );
        cleared.meta.title = None;
        cleared.meta.language = None;

        // Probed with no title; still has none — nothing to clear.
        let untouched = keep(src(0, 2, Kind::Audio, "aac"), Meta::default());

        let joined = project(&["in.mkv"], vec![cleared, untouched]).to_args().join(" ");

        // a:0 gets an explicit empty assignment for each previously-present tag.
        assert!(joined.contains("-metadata:s:a:0 title="), "clears title: {joined}");
        assert!(joined.contains("-metadata:s:a:0 language="), "clears language: {joined}");
        // a:1 never had either, so no metadata is emitted for it at all.
        assert!(!joined.contains("-metadata:s:a:1"), "no spurious clear: {joined}");
    }

    /// A raw/elementary output (extraction target) emits only `-map` + `-c copy`:
    /// no `-metadata`/`-disposition`, which those muxers reject.
    #[test]
    fn raw_output_skips_metadata_and_disposition() {
        let audio = keep(src(0, 1, Kind::Audio, "ac3"), lang("eng"));
        let mut p = project(&["in.mkv"], vec![audio]);
        p.output = PathBuf::from("in.eng.ac3");

        let args = p.to_args();
        let joined = args.join(" ");

        assert_eq!(values_after(&args, "-map"), ["0:1"]);
        assert!(joined.contains("-c:a:0 copy"), "{joined}");
        assert!(!joined.contains("-disposition"), "no disposition: {joined}");
        assert!(!joined.contains("-metadata"), "no metadata: {joined}");
    }

    /// A container title emits a single global `-metadata title=…` (no `:s`), and
    /// only into a real container — a raw/elementary output drops it like the
    /// per-stream tags.
    #[test]
    fn container_title_emitted_globally_for_real_containers() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let mut p = project(&["in.mkv"], vec![v]);
        p.title = Some("My Movie".into());

        let args = p.to_args();
        assert_eq!(values_after(&args, "-metadata"), ["title=My Movie"]);

        // The same project written to a raw output emits no title at all.
        p.output = PathBuf::from("in.h264");
        assert!(values_after(&p.to_args(), "-metadata").is_empty());
    }

    /// An embedded input's offset emits `-itsoffset` *immediately before* that
    /// input's `-i` (and only that one) — the placement ffmpeg requires.
    #[test]
    fn itsoffset_emitted_before_input() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let sub = OutStream::new(src(1, 0, Kind::Subtitle, "subrip"), lang("eng"), Encode::Copy);

        let mut p = project(&["vid.mkv", "subs.srt"], vec![v, sub]);
        p.streams[1].offset_secs = -0.2; // advance the subtitle by 200ms
        let args = p.to_args();

        // The token right before the second `-i` is the offset; the first `-i` has none.
        let i_positions: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == "-i")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(i_positions.len(), 2);
        // The primary has no offset, so its `-i` is the very first token.
        assert_eq!(i_positions[0], 0, "no offset on the primary");
        // The second input is preceded by `-itsoffset <value> -i`.
        assert_eq!(args[i_positions[1] - 2], "-itsoffset");
        assert_eq!(args[i_positions[1] - 1], "-0.200");
        assert_eq!(values_after(&args, "-itsoffset"), ["-0.200"]);
    }

    /// A zero offset (the default for every input) emits nothing.
    #[test]
    fn no_itsoffset_when_zero() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let joined = project(&["in.mkv"], vec![v]).to_args().join(" ");
        assert!(!joined.contains("-itsoffset"), "{joined}");
    }

    /// The offset formats with a fixed 3-decimal `.` — no `0.30000000000000004`.
    #[test]
    fn itsoffset_formats_without_float_noise() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let a = keep(src(1, 0, Kind::Audio, "flac"), Meta::default());
        let mut p = project(&["vid.mkv", "extra.flac"], vec![v, a]);
        p.streams[1].offset_secs = 0.3;
        assert_eq!(values_after(&p.to_args(), "-itsoffset"), ["0.300"]);
    }

    // --- preview clips --------------------------------------------------------

    /// A video plus an embedded subtitle shifted by `offset` seconds.
    fn synced(offset: f64) -> Project {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let sub = OutStream::new(src(1, 0, Kind::Subtitle, "subrip"), lang("eng"), Encode::Copy);
        let mut p = project(&["vid.mkv", "subs.srt"], vec![v, sub]);
        p.streams[1].offset_secs = offset;
        p
    }

    /// Every input gets its own `-ss` right before its `-i`, the clip length goes
    /// before the output, and the output is the preview path.
    #[test]
    fn preview_seeks_each_input_and_limits_length() {
        let args = synced(0.0).preview_args(60.0, 30.0, Path::new("prev.mkv"));
        assert_eq!(input_seeks(&args), ["60.000", "60.000"]);
        for (i, a) in args.iter().enumerate() {
            if a == "-i" {
                assert_eq!(args[i - 2], "-ss", "{args:?}");
            }
        }
        // The output-side `-ss 0` trims what the coarse input seeks let through.
        let n = args.len();
        assert_eq!(&args[n - 5..], ["-ss", "0", "-t", "30.000", "prev.mkv"]);
    }

    /// The `-ss` values placed before an `-i` (input seeks), skipping the
    /// output-side `-ss 0`.
    fn input_seeks(args: &[String]) -> Vec<String> {
        args.windows(3)
            .filter(|w| w[0] == "-ss" && w[2] == "-i")
            .map(|w| w[1].clone())
            .collect()
    }

    /// An offset input is seeked to `start - offset` and takes no `-itsoffset`,
    /// which keeps its relative sync without skipping cues the offset pulls in.
    #[test]
    fn preview_folds_offset_into_the_seek() {
        let args = synced(2.0).preview_args(60.0, 30.0, Path::new("prev.mkv"));
        assert_eq!(input_seeks(&args), ["60.000", "58.000"]);
        assert!(values_after(&args, "-itsoffset").is_empty(), "{args:?}");

        let args = synced(-1.5).preview_args(60.0, 30.0, Path::new("prev.mkv"));
        assert_eq!(input_seeks(&args), ["60.000", "61.500"]);
    }

    /// Near the file start the seek clamps at 0 and the rest of the shift is left
    /// to `-itsoffset`: start 1 s with a +3 s offset → no seek, shift +2 s.
    #[test]
    fn preview_clamped_seek_keeps_the_remaining_shift() {
        let args = synced(3.0).preview_args(1.0, 30.0, Path::new("prev.mkv"));
        assert_eq!(input_seeks(&args), ["1.000"], "only the primary seeks");
        assert_eq!(values_after(&args, "-itsoffset"), ["2.000"]);
    }

    /// A preview re-encodes video (so it starts on time, not at the next
    /// keyframe) but still copies the subtitle.
    #[test]
    fn preview_reencodes_video_and_copies_the_rest() {
        let args = synced(0.0).preview_args(60.0, 30.0, Path::new("prev.mkv"));
        assert_eq!(values_after(&args, "-c:v:0"), ["libx264"]);
        assert_eq!(values_after(&args, "-filter:v:0"), [r"scale=-2:min(ih\,720)"]);
        assert_eq!(values_after(&args, "-c:s:0"), ["copy"]);
    }

    /// The preview path leaves the real command untouched.
    #[test]
    fn preview_does_not_change_to_args() {
        let p = synced(0.25);
        let before = p.to_args();
        let _ = p.preview_args(10.0, 30.0, Path::new("prev.mkv"));
        assert_eq!(p.to_args(), before);
        assert!(!before.contains(&"-ss".to_string()) && !before.contains(&"-t".to_string()));
        assert_eq!(values_after(&before, "-itsoffset"), ["0.250"]);
        assert_eq!(values_after(&before, "-c:v:0"), ["copy"]);
    }

    // --- per-stream offsets on tracks inside a file -------------------------

    /// A video, two audio tracks and a subtitle, all in one file.
    fn movie() -> Project {
        project(
            &["movie.mkv"],
            vec![
                keep(src(0, 0, Kind::Video, "h264"), Meta::default()),
                keep(src(0, 1, Kind::Audio, "aac"), lang("eng")),
                keep(src(0, 2, Kind::Audio, "aac"), lang("jpn")),
                keep(src(0, 3, Kind::Subtitle, "subrip"), lang("eng")),
            ],
        )
    }

    /// Shifting one track inside the primary reads the file a second time with
    /// the offset and maps just that track from it; the rest stay on input 0.
    #[test]
    fn shifted_track_in_primary_maps_from_a_second_read() {
        let mut p = movie();
        p.streams[3].offset_secs = 1.5;
        let args = p.to_args();
        assert_eq!(&args[..5], ["-i", "movie.mkv", "-itsoffset", "1.500", "-i"]);
        assert_eq!(args[5], "movie.mkv");
        assert_eq!(values_after(&args, "-map"), ["0:0", "0:1", "0:2", "1:3"]);
    }

    /// Tracks sharing an offset share one extra read; a different offset gets
    /// its own.
    #[test]
    fn one_extra_read_per_distinct_offset() {
        let mut p = movie();
        p.streams[1].offset_secs = -0.5;
        p.streams[3].offset_secs = -0.5;
        p.streams[2].offset_secs = 0.25;
        let args = p.to_args();
        assert_eq!(values_after(&args, "-itsoffset"), ["-0.500", "0.250"]);
        assert_eq!(values_after(&args, "-i").len(), 3);
        assert_eq!(values_after(&args, "-map"), ["0:0", "1:1", "2:2", "1:3"]);
    }

    /// A removed shifted track doesn't cause an extra read.
    #[test]
    fn removed_shifted_track_adds_no_read() {
        let mut p = movie();
        p.streams[3].offset_secs = 1.0;
        p.streams[3].removed = true;
        let args = p.to_args();
        assert_eq!(values_after(&args, "-i"), ["movie.mkv"]);
        assert!(values_after(&args, "-itsoffset").is_empty());
    }

    /// A preview seeks each read on its own: the shifted read of the same file
    /// seeks to `start - offset`.
    #[test]
    fn preview_seeks_the_shifted_read_separately() {
        let mut p = movie();
        p.streams[3].offset_secs = 2.0;
        let args = p.preview_args(60.0, 30.0, Path::new("prev.mkv"));
        assert_eq!(input_seeks(&args), ["60.000", "58.000"]);
        assert_eq!(values_after(&args, "-map"), ["0:0", "0:1", "0:2", "1:3"]);
    }
}
