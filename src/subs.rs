//! Reading a subtitle stream's cues as text, for the in-app cue viewer.
//!
//! Rather than parse every subtitle format ourselves (ASS, WebVTT, mov_text,
//! SubRip, ...), we let ffmpeg convert the stream to SRT on stdout and parse
//! only that. SRT is the lowest common denominator: timing plus text, which is
//! all a "is this translated / timed right?" check needs. Styling is lost, and
//! the few inline tags ffmpeg keeps (`<i>`, `{\an8}`, ...) are stripped for
//! display.
//!
//! Image-based subtitles (PGS, VobSub, DVB) have no text to convert, so they are
//! recognized up front and never sent to ffmpeg.
//!
//! The read runs on a plain thread, like `run.rs`: ffmpeg has to demux the whole
//! container to collect every cue, which can take seconds on a large file.

use crate::run::no_window;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

/// One subtitle cue, timed in the source stream's own timeline (no offset).
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    /// When the cue appears, in seconds.
    pub start_secs: f64,
    /// When the cue disappears, in seconds.
    pub end_secs: f64,
    /// The cue's text with markup stripped; lines are joined with `\n`.
    pub text: String,
}

/// Whether `codec` (ffprobe's `codec_name`) carries text that can be shown as
/// cues. Image-based formats are bitmaps, with nothing for ffmpeg to convert.
pub fn is_text_subtitle(codec: &str) -> bool {
    !matches!(
        codec,
        "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle" | "dvb_teletext" | "xsub"
    )
}

/// Read the cues of stream `stream_index` (absolute, as ffprobe reports it) of
/// `input` on a background thread. The single result arrives over the channel.
pub fn read_cues(ffmpeg: &str, input: &Path, stream_index: usize) -> Receiver<Result<Vec<Cue>, String>> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(input)
        .args(["-map", &format!("0:{stream_index}"), "-c:s", "srt", "-f", "srt", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);

    let ffmpeg = ffmpeg.to_owned();
    let (tx, rx) = channel();
    thread::spawn(move || {
        let result = match cmd.output() {
            Err(e) => Err(format!("could not launch ffmpeg (`{ffmpeg}`): {e}")),
            Ok(out) if !out.status.success() => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let last = stderr.lines().rfind(|l| !l.trim().is_empty());
                Err(last.unwrap_or("ffmpeg failed to read the subtitles").trim().to_owned())
            }
            Ok(out) => Ok(parse_srt(&String::from_utf8_lossy(&out.stdout))),
        };
        let _ = tx.send(result);
    });
    rx
}

/// Parse SRT text into cues. Lenient by design: blocks are split on blank
/// lines, the numeric counter is optional, and a block without a readable
/// `start --> end` line is skipped rather than failing the whole file.
pub fn parse_srt(text: &str) -> Vec<Cue> {
    let text = text.replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n") {
        let mut lines = block.lines().map(str::trim_end).skip_while(|l| l.trim().is_empty());
        let Some(mut first) = lines.next() else { continue };
        // The counter line is optional: if this isn't the timing line, try the next.
        if !first.contains("-->") {
            match lines.next() {
                Some(l) => first = l,
                None => continue,
            }
        }
        let Some((start, end)) = parse_timing(first) else { continue };
        let body: Vec<String> = lines.map(strip_markup).collect();
        let text = body.join("\n").trim().to_owned();
        cues.push(Cue {
            start_secs: start,
            end_secs: end,
            text,
        });
    }
    cues
}

/// `00:01:02,500 --> 00:01:04,000` → `(62.5, 64.0)`. Anything after the end
/// time (SRT position hints) is ignored.
fn parse_timing(line: &str) -> Option<(f64, f64)> {
    let (a, b) = line.split_once("-->")?;
    let b = b.split_whitespace().next()?;
    Some((parse_srt_time(a.trim())?, parse_srt_time(b)?))
}

/// `HH:MM:SS,mmm` (or with `.`) → seconds.
fn parse_srt_time(s: &str) -> Option<f64> {
    let s = s.replace(',', ".");
    let mut parts = s.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Drop HTML-style tags (`<i>`, `</font>`) and ASS override blocks (`{\an8}`),
/// keeping the readable text.
fn strip_markup(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut close: Option<char> = None;
    for c in line.chars() {
        match close {
            Some(end) if c == end => close = None,
            Some(_) => {}
            None if c == '<' => close = Some('>'),
            None if c == '{' => close = Some('}'),
            None => out.push(c),
        }
    }
    out
}

/// Format seconds as `HH:MM:SS.mmm` for display. Negative times (a cue pushed
/// before the start by a negative offset) keep a leading `-`.
pub fn format_time(secs: f64) -> String {
    let sign = if secs < 0.0 { "-" } else { "" };
    let ms = (secs.abs() * 1000.0).round() as u64;
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{sign}{h:02}:{m:02}:{s:02}.{ms:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_cues_with_crlf() {
        let srt = "1\r\n00:00:01,000 --> 00:00:02,500\r\nHello\r\nthere\r\n\r\n2\r\n00:01:02,250 --> 00:01:04,000\r\nBye\r\n";
        let cues = parse_srt(srt);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start_secs, 1.0);
        assert_eq!(cues[0].end_secs, 2.5);
        assert_eq!(cues[0].text, "Hello\nthere");
        assert_eq!(cues[1].start_secs, 62.25);
        assert_eq!(cues[1].text, "Bye");
    }

    #[test]
    fn strips_tags_and_ass_overrides() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\n{\\an8}<i>Up</i> <font color=\"red\">top</font>\n";
        assert_eq!(parse_srt(srt)[0].text, "Up top");
    }

    #[test]
    fn skips_malformed_blocks_and_tolerates_missing_counter() {
        let srt = "1\nnot a timing line\nText\n\n00:00:03,000 --> 00:00:04,000 X1:10\nKept\n\n\n\n";
        let cues = parse_srt(srt);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start_secs, 3.0);
        assert_eq!(cues[0].text, "Kept");
    }

    #[test]
    fn bitmap_subtitles_are_not_text() {
        assert!(!is_text_subtitle("hdmv_pgs_subtitle"));
        assert!(!is_text_subtitle("dvd_subtitle"));
        assert!(is_text_subtitle("ass"));
        assert!(is_text_subtitle("subrip"));
    }

    #[test]
    fn formats_times() {
        assert_eq!(format_time(62.25), "00:01:02.250");
        assert_eq!(format_time(3725.0), "01:02:05.000");
        assert_eq!(format_time(-1.5), "-00:00:01.500");
    }
}
