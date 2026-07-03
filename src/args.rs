//! Serializing a `Project` into ffmpeg arguments.
//!
//! The single most error-prone rule ffmpeg imposes: **output stream indices are
//! assigned per type, in `-map` order.** The 2nd audio stream mapped becomes
//! `:a:1` for every later option, regardless of its source index. So we walk the
//! streams in output order and keep a per-type counter; every `-c` / `-metadata`
//! / `-disposition` uses that output index, not the source index.

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

impl Project {
    /// Render this project as the argument vector for `ffmpeg` (the program name
    /// itself is not included). Passed straight to `std::process::Command`, so no
    /// shell quoting is needed or wanted.
    ///
    /// Deliberately does **not** emit `-y`/`-n`: overwrite policy is decided by
    /// the run layer after an existence check, not baked into the command here.
    pub fn to_args(&self) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();

        // Inputs, in order — their position defines the `-i` index streams refer to.
        // A synced input gets `-itsoffset <secs>` right before its `-i`. We gate on
        // integer milliseconds so tiny float noise never emits a spurious offset,
        // and format with a fixed `.` decimal (locale-independent, no `1e-1` forms).
        for input in &self.inputs {
            let ms = (input.offset_secs * 1000.0).round() as i64;
            if ms != 0 {
                a.push("-itsoffset".into());
                a.push(format!("{:.3}", ms as f64 / 1000.0)); // e.g. "0.200", "-0.150"
            }
            a.push("-i".into());
            a.push(input.path.display().to_string());
        }

        // Only streams not marked for removal reach the output; a removed stream
        // is simply never mapped (and doesn't count toward per-type renumbering).
        let live: Vec<&OutStream> = self.streams.iter().filter(|s| !s.removed).collect();

        // Maps first, in output order. This is what actually fixes ordering:
        // ffmpeg emits streams in the order they're mapped.
        for s in &live {
            a.push("-map".into());
            a.push(format!("{}:{}", s.source.input, s.source.index));
        }

        // Raw/elementary outputs (extraction) reject per-stream tags entirely.
        let emit_tags = output_takes_stream_tags(&self.output);

        // Per-type output counter: the Nth audio we mapped is `:a:<n>`.
        let mut counter: HashMap<char, usize> = HashMap::new();
        for s in &live {
            let c = s.source.kind.spec();
            let n = *counter.entry(c).or_insert(0);
            let spec = format!("{c}:{n}"); // e.g. "a:0"

            // Codec: copy for everything except the audio-conversion path.
            match &s.encode {
                Encode::Copy => {
                    a.push(format!("-c:{spec}"));
                    a.push("copy".into());
                }
                Encode::Audio {
                    codec,
                    bitrate_kbps,
                    channels,
                } => {
                    a.push(format!("-c:{spec}"));
                    a.push(codec.clone());
                    if let Some(b) = bitrate_kbps {
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
                *counter.get_mut(&c).unwrap() += 1;
                continue;
            }

            // Metadata. `None` emits nothing, so the original tag passes through
            // the copy untouched.
            if let Some(l) = &s.meta.language {
                a.push(format!("-metadata:s:{spec}"));
                a.push(format!("language={l}"));
            }
            if let Some(t) = &s.meta.title {
                a.push(format!("-metadata:s:{spec}"));
                a.push(format!("title={t}"));
            }

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

            *counter.get_mut(&c).unwrap() += 1;
        }

        a.push(self.output.display().to_string());
        a
    }
}

#[cfg(test)]
mod tests {
    use crate::model::*;
    use std::path::PathBuf;

    // --- small builders to keep the tests readable ---------------------------

    fn src(input: usize, index: usize, kind: Kind, codec: &str) -> Source {
        Source {
            input,
            index,
            kind,
            codec: codec.into(),
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
                bitrate_kbps: Some(192),
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

    /// An embedded input's offset emits `-itsoffset` *immediately before* that
    /// input's `-i` (and only that one) — the placement ffmpeg requires.
    #[test]
    fn itsoffset_emitted_before_input() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let sub = OutStream::new(src(1, 0, Kind::Subtitle, "subrip"), lang("eng"), Encode::Copy);

        let mut p = project(&["vid.mkv", "subs.srt"], vec![v, sub]);
        p.inputs[1].offset_secs = -0.2; // advance the subtitle by 200ms
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
        p.inputs[1].offset_secs = 0.3;
        assert_eq!(values_after(&p.to_args(), "-itsoffset"), ["0.300"]);
    }
}
