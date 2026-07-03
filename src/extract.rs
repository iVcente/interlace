//! Extracting a single stream into its own file.
//!
//! Extraction is just another `Project`: one input, one mapped stream, always
//! `-c copy`, written to a file whose extension matches the stream's codec. The
//! serializer (`args.rs`) already knows that raw/elementary outputs like `.ac3`
//! or `.srt` can't carry per-stream metadata/disposition, so building the right
//! *output path* here is all it takes to get a correct command — the two pieces
//! compose. Container fallbacks (`.mka`/`.mks`/`.mkv`) are used when a codec has
//! no clean elementary muxer; those do carry the tags, which is fine.

use crate::model::{Encode, Input, Kind, OutStream, Project, Source};
use std::path::{Path, PathBuf};

impl Project {
    /// Build a single-stream extraction project for `streams[idx]`, or `None`
    /// for an out-of-range index or an attachment/data stream (those are pulled
    /// out with `-dump_attachment`, not stream mapping — out of scope here).
    pub fn extract(&self, idx: usize) -> Option<Project> {
        let s = self.streams.get(idx)?;
        if matches!(s.source.kind, Kind::Attachment | Kind::Data) {
            return None;
        }
        let input = self.inputs.get(s.source.input)?.path.clone();
        let ext = natural_extension(s.source.kind, &s.source.codec);
        let output = extract_output_path(&input, s, idx, ext);

        // One input, one stream: the source keeps its absolute index (same file)
        // but now refers to input 0 of this fresh project.
        let stream = OutStream::new(
            Source {
                input: 0,
                index: s.source.index,
                kind: s.source.kind,
                codec: s.source.codec.clone(),
            },
            s.meta.clone(),
            Encode::Copy, // extraction never re-encodes
        );

        // A fresh single-input project: the offset resets to 0 (nothing to sync
        // against once the stream stands alone).
        Some(Project {
            inputs: vec![Input::new(input)],
            streams: vec![stream],
            output,
            duration_secs: self.duration_secs,
        })
    }
}

/// The file extension that best matches a stream's codec for a copied extract.
/// Falls back to a single-stream Matroska container (`.mka`/`.mks`/`.mkv`) when a
/// codec has no clean raw/elementary muxer — copy always works into Matroska.
fn natural_extension(kind: Kind, codec: &str) -> &'static str {
    match kind {
        Kind::Audio => match codec {
            "aac" => "aac",
            "ac3" => "ac3",
            "eac3" => "eac3",
            "dts" => "dts",
            "mp3" => "mp3",
            "flac" => "flac",
            "opus" => "opus",
            "vorbis" => "ogg",
            "truehd" => "thd",
            "alac" => "m4a",
            c if c.starts_with("pcm_") => "wav",
            _ => "mka",
        },
        Kind::Video => match codec {
            "h264" => "h264",
            "hevc" => "hevc",
            "mpeg2video" => "m2v",
            _ => "mkv",
        },
        Kind::Subtitle => match codec {
            "subrip" => "srt",
            "ass" | "ssa" => "ass",
            "webvtt" => "vtt",
            "hdmv_pgs_subtitle" => "sup",
            _ => "mks",
        },
        // Excluded above, but keep the match total.
        Kind::Attachment | Kind::Data => "bin",
    }
}

/// `movie.mkv` + eng audio at idx 1 → `movie.eng.ac3` next to the input.
/// Falls back to `track<idx>` when the stream has no language tag.
fn extract_output_path(input: &Path, s: &OutStream, idx: usize, ext: &str) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    let label = s
        .meta
        .language
        .clone()
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| format!("track{idx}"));
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem}.{label}.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Meta;

    fn stream(input: usize, index: usize, kind: Kind, codec: &str, lang: Option<&str>) -> OutStream {
        OutStream::new(
            Source { input, index, kind, codec: codec.into() },
            Meta { language: lang.map(Into::into), ..Default::default() },
            Encode::Copy,
        )
    }

    fn project(streams: Vec<OutStream>) -> Project {
        Project {
            inputs: vec![Input::new(PathBuf::from("/media/movie.mkv"))],
            streams,
            output: PathBuf::from("/media/movie.remux.mkv"),
            duration_secs: Some(90.0),
        }
    }

    #[test]
    fn extract_audio_picks_codec_extension_and_copies() {
        let p = project(vec![
            stream(0, 0, Kind::Video, "hevc", None),
            stream(0, 1, Kind::Audio, "ac3", Some("eng")),
        ]);
        let x = p.extract(1).unwrap();

        assert_eq!(x.streams.len(), 1);
        assert!(matches!(x.streams[0].encode, Encode::Copy));
        assert!(x.output.to_string_lossy().ends_with("movie.eng.ac3"));

        // The command maps only the chosen absolute index, with no tags.
        let joined = x.to_args().join(" ");
        assert!(joined.contains("-map 0:1"), "{joined}");
        assert!(!joined.contains("-disposition"), "{joined}");
    }

    #[test]
    fn extract_subtitle_maps_to_srt() {
        let p = project(vec![stream(0, 3, Kind::Subtitle, "subrip", Some("spa"))]);
        let x = p.extract(0).unwrap();
        assert!(x.output.to_string_lossy().ends_with("movie.spa.srt"));
    }

    #[test]
    fn extract_untagged_stream_uses_track_label() {
        let p = project(vec![stream(0, 2, Kind::Audio, "flac", None)]);
        let x = p.extract(0).unwrap();
        assert!(x.output.to_string_lossy().ends_with("movie.track0.flac"));
    }

    #[test]
    fn extract_attachment_is_unsupported() {
        let p = project(vec![stream(0, 4, Kind::Attachment, "ttf", None)]);
        assert!(p.extract(0).is_none());
    }

    #[test]
    fn extract_resets_offset() {
        // A second input with a sync offset, and a stream drawn from it.
        let mut p = project(vec![stream(1, 0, Kind::Audio, "flac", Some("eng"))]);
        p.inputs.push(Input {
            path: PathBuf::from("/media/extra.flac"),
            offset_secs: -0.2,
        });
        let x = p.extract(0).unwrap();
        // The extracted single-input project carries no offset.
        assert_eq!(x.inputs[0].offset_secs, 0.0);
        assert!(!x.to_args().join(" ").contains("-itsoffset"));
    }

    #[test]
    fn unknown_codec_falls_back_to_matroska_and_keeps_tags() {
        let p = project(vec![stream(0, 1, Kind::Audio, "cook", Some("rus"))]);
        let x = p.extract(0).unwrap();
        assert!(x.output.to_string_lossy().ends_with("movie.rus.mka"));
        // .mka is a real container, so tags come back.
        assert!(x.to_args().join(" ").contains("-disposition"));
    }
}
