//! Serializing a `Project` into ffmpeg arguments.
//!
//! The single most error-prone rule ffmpeg imposes: **output stream indices are
//! assigned per type, in `-map` order.** The 2nd audio stream mapped becomes
//! `:a:1` for every later option, regardless of its source index. So we walk the
//! streams in output order and keep a per-type counter; every `-c` / `-metadata`
//! / `-disposition` uses that output index, not the source index.

use crate::model::{Encode, Kind, Project};
use std::collections::HashMap;

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
        for input in &self.inputs {
            a.push("-i".into());
            a.push(input.display().to_string());
        }

        // Maps first, in output order. This is what actually fixes ordering:
        // ffmpeg emits streams in the order they're mapped.
        for s in &self.streams {
            a.push("-map".into());
            a.push(format!("{}:{}", s.source.input, s.source.index));
        }

        // Per-type output counter: the Nth audio we mapped is `:a:<n>`.
        let mut counter: HashMap<char, usize> = HashMap::new();
        for s in &self.streams {
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
            // types — so stop here for them.
            if matches!(s.source.kind, Kind::Attachment | Kind::Data) {
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
        OutStream {
            source,
            meta,
            encode: Encode::Copy,
        }
    }

    fn lang(l: &str) -> Meta {
        Meta {
            language: Some(l.into()),
            ..Default::default()
        }
    }

    fn project(inputs: &[&str], streams: Vec<OutStream>) -> Project {
        Project {
            inputs: inputs.iter().map(PathBuf::from).collect(),
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

    /// Audio convert: the one re-encode path, on the correct output index.
    #[test]
    fn audio_convert_emits_codec_bitrate_channels() {
        let v = keep(src(0, 0, Kind::Video, "h264"), Meta::default());
        let audio = OutStream {
            source: src(0, 1, Kind::Audio, "flac"),
            meta: Meta::default(),
            encode: Encode::Audio {
                codec: "aac".into(),
                bitrate_kbps: Some(192),
                channels: Some(2),
            },
        };

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
        let commentary = OutStream {
            source: src(1, 0, Kind::Audio, "flac"),
            meta: Meta {
                title: Some("Commentary".into()),
                ..Default::default()
            },
            encode: Encode::Copy,
        };

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
}
