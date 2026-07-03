//! Pre-flight container/codec compatibility checks.
//!
//! This is deliberately *rule-based*, not a model of everything ffmpeg can do:
//! we catch the handful of common mismatches that produce cryptic ffmpeg errors
//! (an ASS subtitle bound for an MP4, a font attachment bound for anything but
//! Matroska, an AAC track in a WebM) and turn them into a legible message plus a
//! concrete suggestion. ffmpeg remains the final authority — these are hints
//! surfaced *before* a run, not a gate that blocks it.
//!
//! The "effective" codec of a stream is its conversion target when it's being
//! re-encoded, otherwise the source codec that will be copied through.

use crate::model::{Encode, Kind, OutStream, Project};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One compatibility finding against the current project.
#[derive(Debug, Clone)]
pub struct Issue {
    pub severity: Severity,
    /// Output-stream index this concerns, if it's about a specific stream.
    pub stream: Option<usize>,
    pub message: String,
    /// A concrete next step, if we have one to offer.
    pub suggestion: Option<String>,
}

/// The output containers whose rules we know something about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Matroska,
    Mp4,
    WebM,
    /// Anything we don't have rules for — we skip codec checks for these.
    Other,
}

fn container_of(project: &Project) -> Container {
    match project
        .output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("mkv") | Some("mka") => Container::Matroska,
        Some("mp4") | Some("m4v") | Some("m4a") | Some("mov") => Container::Mp4,
        Some("webm") => Container::WebM,
        _ => Container::Other,
    }
}

/// The codec that will actually land in the output: the conversion target when
/// re-encoding, otherwise the copied-through source codec.
fn effective_codec(s: &OutStream) -> &str {
    match &s.encode {
        Encode::Copy => &s.source.codec,
        Encode::Audio { codec, .. } => codec,
    }
}

/// Run every rule against `project`, returning findings in stream order with
/// errors ahead of warnings.
pub fn validate(project: &Project) -> Vec<Issue> {
    let mut issues = Vec::new();

    if project.streams.is_empty() {
        issues.push(Issue {
            severity: Severity::Error,
            stream: None,
            message: "No streams selected — the output would be empty.".into(),
            suggestion: Some("Load a file, or keep at least one stream.".into()),
        });
        return issues;
    }

    let container = container_of(project);
    if container == Container::Other {
        return issues; // unknown container: don't guess at its rules
    }

    for (i, s) in project.streams.iter().enumerate() {
        if let Some(issue) = check_stream(container, i, s) {
            issues.push(issue);
        }
    }

    // Errors first so the most actionable findings lead.
    issues.sort_by_key(|issue| match issue.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    });
    issues
}

fn check_stream(container: Container, i: usize, s: &OutStream) -> Option<Issue> {
    match container {
        Container::Matroska | Container::Other => None, // Matroska takes ~everything
        Container::Mp4 => check_mp4(i, s),
        Container::WebM => check_webm(i, s),
    }
}

/// MP4/MOV: text subtitles must be `mov_text`; fonts/attachments aren't stored.
fn check_mp4(i: usize, s: &OutStream) -> Option<Issue> {
    let codec = effective_codec(s);
    match s.source.kind {
        Kind::Subtitle if !is_mp4_subtitle(codec) => Some(Issue {
            severity: Severity::Error,
            stream: Some(i),
            message: format!("MP4 can't carry a `{codec}` subtitle."),
            suggestion: Some(
                "MP4 only supports mov_text (tx3g) subtitles — use an .mkv output, \
                 or remove this stream."
                    .into(),
            ),
        }),
        Kind::Attachment => Some(Issue {
            severity: Severity::Error,
            stream: Some(i),
            message: "MP4 can't store font/attachment streams.".into(),
            suggestion: Some("Use an .mkv output to keep attachments, or remove this stream.".into()),
        }),
        Kind::Audio if codec.starts_with("pcm_") => Some(Issue {
            severity: Severity::Warning,
            stream: Some(i),
            message: format!("PCM audio (`{codec}`) is poorly supported in MP4."),
            suggestion: Some("Convert this audio to aac, or use an .mkv output.".into()),
        }),
        _ => None,
    }
}

/// WebM only permits VP8/VP9/AV1 video, Vorbis/Opus audio, and WebVTT subs.
fn check_webm(i: usize, s: &OutStream) -> Option<Issue> {
    let codec = effective_codec(s);
    let bad = |what: &str, allowed: &str, hint: &str| {
        Some(Issue {
            severity: Severity::Error,
            stream: Some(i),
            message: format!("WebM can't carry a `{codec}` {what} stream."),
            suggestion: Some(format!("WebM only allows {allowed}. {hint}")),
        })
    };
    match s.source.kind {
        Kind::Video if !matches!(codec, "vp8" | "vp9" | "av1") => {
            bad("video", "VP8, VP9, or AV1 video", "Use an .mkv output instead.")
        }
        Kind::Audio if !matches!(codec, "vorbis" | "opus") => {
            bad("audio", "Vorbis or Opus audio", "Convert this audio to opus, or use an .mkv output.")
        }
        Kind::Subtitle if codec != "webvtt" => {
            bad("subtitle", "WebVTT subtitles", "Use an .mkv output instead.")
        }
        Kind::Attachment | Kind::Data => bad("attachment/data", "video, audio, and WebVTT streams", "Use an .mkv output instead."),
        _ => None,
    }
}

/// The subtitle codecs MP4 accepts. (ffmpeg calls the tx3g codec `mov_text`.)
fn is_mp4_subtitle(codec: &str) -> bool {
    matches!(codec, "mov_text" | "tx3g")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Input, Meta, Source};
    use std::path::PathBuf;

    fn stream(kind: Kind, codec: &str, encode: Encode) -> OutStream {
        OutStream::new(
            Source { input: 0, index: 0, kind, codec: codec.into() },
            Meta::default(),
            encode,
        )
    }

    fn project(output: &str, streams: Vec<OutStream>) -> Project {
        Project {
            inputs: vec![Input::new(PathBuf::from("in.mkv"))],
            streams,
            output: PathBuf::from(output),
            duration_secs: None,
        }
    }

    #[test]
    fn ass_subtitle_in_mp4_is_an_error_with_suggestion() {
        let p = project("out.mp4", vec![
            stream(Kind::Video, "h264", Encode::Copy),
            stream(Kind::Subtitle, "ass", Encode::Copy),
        ]);
        let issues = validate(&p);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
        assert_eq!(issues[0].stream, Some(1));
        assert!(issues[0].suggestion.as_ref().unwrap().contains("mov_text"));
    }

    #[test]
    fn attachment_in_mp4_is_an_error() {
        let p = project("out.mp4", vec![stream(Kind::Attachment, "ttf", Encode::Copy)]);
        let issues = validate(&p);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn aac_audio_in_webm_is_an_error_but_opus_conversion_is_clean() {
        // Copied AAC → rejected by WebM.
        let bad = project("out.webm", vec![
            stream(Kind::Video, "vp9", Encode::Copy),
            stream(Kind::Audio, "aac", Encode::Copy),
        ]);
        assert_eq!(validate(&bad).len(), 1);

        // Same track, but converted to opus → effective codec is now allowed.
        let good = project("out.webm", vec![
            stream(Kind::Video, "vp9", Encode::Copy),
            stream(Kind::Audio, "aac", Encode::Audio {
                codec: "opus".into(),
                bitrate_kbps: Some(192),
                channels: None,
            }),
        ]);
        assert!(validate(&good).is_empty());
    }

    #[test]
    fn matroska_accepts_everything() {
        let p = project("out.mkv", vec![
            stream(Kind::Video, "h264", Encode::Copy),
            stream(Kind::Audio, "flac", Encode::Copy),
            stream(Kind::Subtitle, "ass", Encode::Copy),
            stream(Kind::Attachment, "ttf", Encode::Copy),
        ]);
        assert!(validate(&p).is_empty());
    }

    #[test]
    fn unknown_container_is_not_second_guessed() {
        let p = project("out.avi", vec![stream(Kind::Subtitle, "ass", Encode::Copy)]);
        assert!(validate(&p).is_empty());
    }

    #[test]
    fn empty_project_reports_one_error() {
        let p = project("out.mkv", vec![]);
        let issues = validate(&p);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
    }
}
