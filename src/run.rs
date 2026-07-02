//! Running ffmpeg and streaming progress back to the caller.
//!
//! We spawn ffmpeg as a child process with `-progress pipe:1`, which makes it
//! emit machine-readable `key=value` progress blocks on stdout — far easier to
//! parse than the human-readable stats it writes to stderr. A background thread
//! reads those blocks and forwards `Progress` values over an `mpsc` channel; a
//! second thread drains stderr so we can surface *why* a run failed. The caller
//! (a CLI today, the egui UI later) just consumes `RunUpdate`s from the channel.
//!
//! No async runtime: plain threads + a channel keep the dependency surface small.

use crate::model::Project;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

/// A single progress sample, one per ffmpeg progress block.
#[derive(Debug, Clone)]
pub struct Progress {
    /// Position in the output measured in seconds.
    pub out_time_secs: f64,
    /// Bytes written so far, if ffmpeg reported it.
    pub total_size: Option<u64>,
    /// Encoding speed as a multiple of realtime (e.g. `1.5` for `1.5x`).
    pub speed: Option<f64>,
    /// Completion in `0.0..=1.0`, if the input duration was known up front.
    pub fraction: Option<f64>,
}

/// Details of a failed run, assembled for a legible error message.
#[derive(Debug, Clone)]
pub struct RunFailure {
    pub code: Option<i32>,
    /// The tail of ffmpeg's stderr — where the actual error almost always is.
    pub stderr_tail: String,
}

/// What the run thread reports over the channel.
#[derive(Debug, Clone)]
pub enum RunUpdate {
    Progress(Progress),
    Finished { result: Result<(), RunFailure> },
}

/// Spawn ffmpeg to render `project` and return a channel of updates.
///
/// `ffmpeg` is the program to invoke (a bare `"ffmpeg"` resolves via PATH).
/// `overwrite` reflects the user's confirmed decision: with it `false`, an
/// existing output is an error caught *here* rather than a hang inside ffmpeg's
/// interactive prompt. Either way we pass `-nostdin` so ffmpeg can never block
/// waiting on input.
pub fn run(
    ffmpeg: &str,
    project: &Project,
    overwrite: bool,
) -> Result<Receiver<RunUpdate>, String> {
    // Pre-check existence ourselves (the UI will confirm before calling us).
    if !overwrite && project.output.exists() {
        return Err(format!(
            "output already exists: {}",
            project.output.display()
        ));
    }

    // Global options first, then the project's own `-i … -map … output`.
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "warning".into(), // drop per-frame noise, keep warnings + errors
        "-nostdin".into(),
        "-progress".into(),
        "pipe:1".into(),
    ];
    if overwrite {
        args.push("-y".into());
    }
    args.extend(project.to_args());

    spawn(ffmpeg, args, project.duration_secs)
}

/// Run a user-edited command verbatim — the "escape hatch". `args` are the
/// tokens *after* the program name (see [`tokenize`]); the caller supplies the
/// ffmpeg binary, so the configured path still wins over whatever program name
/// the user typed. We prepend only the flags needed to drive the progress bar
/// and keep ffmpeg from blocking on input — everything else (including `-y` and
/// the output path) is left exactly as the user wrote it.
pub fn run_raw(
    ffmpeg: &str,
    user_args: Vec<String>,
    duration: Option<f64>,
) -> Result<Receiver<RunUpdate>, String> {
    if user_args.is_empty() {
        return Err("the edited command is empty".into());
    }
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-progress".into(),
        "pipe:1".into(),
    ];
    args.extend(user_args);
    spawn(ffmpeg, args, duration)
}

/// Launch ffmpeg with the fully-assembled `args` and stream updates back.
/// Shared by [`run`] (model-driven) and [`run_raw`] (escape hatch).
fn spawn(ffmpeg: &str, args: Vec<String>, duration: Option<f64>) -> Result<Receiver<RunUpdate>, String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not launch ffmpeg (`{ffmpeg}`): {e}"))?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // Drain stderr on its own thread so a full pipe can't deadlock the child.
    let stderr_handle = thread::spawn(move || read_to_lines(stderr));

    let (tx, rx) = channel();
    thread::spawn(move || {
        // Forward every progress block as it arrives.
        drive_progress(BufReader::new(stdout), duration, |p| {
            let _ = tx.send(RunUpdate::Progress(p));
        });

        // stdout closed → the process is finishing; collect its verdict.
        let status = child.wait();
        let stderr_lines = stderr_handle.join().unwrap_or_default();
        let result = match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(RunFailure {
                code: s.code(),
                stderr_tail: tail(&stderr_lines, 20),
            }),
            Err(e) => Err(RunFailure {
                code: None,
                stderr_tail: format!("could not wait on ffmpeg: {e}"),
            }),
        };
        let _ = tx.send(RunUpdate::Finished { result });
    });

    Ok(rx)
}

/// Probe a binary by running `<program> -version`, returning its first line
/// (e.g. `ffmpeg version 8.1 …`) on success or a legible error if it can't be
/// launched or exits non-zero. Used at startup to detect ffmpeg/ffprobe and to
/// validate a user-supplied override path.
pub fn version(program: &str) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);

    let output = cmd
        .output()
        .map_err(|e| format!("not found (`{program}`): {e}"))?;
    if !output.status.success() {
        return Err(format!("`{program} -version` exited with an error"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().next().unwrap_or("").trim().to_string())
}

/// Split a command line into argv tokens, honoring `'…'` and `"…"` quoting so a
/// path with spaces survives as one token. Backslashes are kept literal (Windows
/// paths are the common case), so there's no escape processing. An empty quoted
/// string `""` yields an empty token.
pub fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;

    for c in input.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None; // close the quote; token may continue
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                    in_token = true; // even `""` is a (empty) token
                } else if c.is_whitespace() {
                    if in_token {
                        out.push(std::mem::take(&mut cur));
                        in_token = false;
                    }
                } else {
                    cur.push(c);
                    in_token = true;
                }
            }
        }
    }
    if in_token {
        out.push(cur);
    }
    out
}

/// Accumulates the `key=value` lines of one progress block until its terminating
/// `progress=` line, then flushes a `Progress`.
#[derive(Default)]
struct Acc {
    out_time_us: Option<u64>,
    total_size: Option<u64>,
    speed: Option<f64>,
}

impl Acc {
    fn to_progress(&self, duration: Option<f64>) -> Progress {
        let out_time_secs = self.out_time_us.map(|us| us as f64 / 1_000_000.0).unwrap_or(0.0);
        let fraction = duration
            .filter(|d| *d > 0.0)
            .map(|d| (out_time_secs / d).clamp(0.0, 1.0));
        Progress {
            out_time_secs,
            total_size: self.total_size,
            speed: self.speed,
            fraction,
        }
    }
}

/// Parse ffmpeg's `-progress` stream, calling `emit` once per block. Kept
/// generic over the reader so it can be unit-tested against a canned stream.
fn drive_progress<R: BufRead>(reader: R, duration: Option<f64>, mut emit: impl FnMut(Progress)) {
    let mut acc = Acc::default();
    for line in reader.lines().map_while(Result::ok) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            // Newer ffmpeg reports microseconds; "N/A" early on parses to None.
            "out_time_us" => acc.out_time_us = value.parse().ok(),
            "total_size" => acc.total_size = value.parse().ok(),
            "speed" => acc.speed = parse_speed(value),
            "progress" => {
                emit(acc.to_progress(duration));
                let ended = value == "end";
                acc = Acc::default();
                if ended {
                    break;
                }
            }
            _ => {}
        }
    }
}

/// `"1.53x"` -> `Some(1.53)`; `"N/A"` -> `None`. Tolerates padding ffmpeg
/// sometimes adds, e.g. `" 1.5x"`.
fn parse_speed(v: &str) -> Option<f64> {
    let v = v.trim();
    v.strip_suffix('x').unwrap_or(v).trim().parse().ok()
}

fn read_to_lines(stream: impl Read) -> Vec<String> {
    BufReader::new(stream)
        .lines()
        .map_while(Result::ok)
        .collect()
}

/// The last `n` non-empty lines, joined — that's where ffmpeg's real error is.
fn tail(lines: &[String], n: usize) -> String {
    let kept: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let start = kept.len().saturating_sub(n);
    kept[start..]
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Suppress the flashing console window when ffmpeg is launched from a GUI.
#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_window(_cmd: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_speed_and_na() {
        assert_eq!(parse_speed("1.53x"), Some(1.53));
        assert_eq!(parse_speed(" 2x "), Some(2.0));
        assert_eq!(parse_speed("N/A"), None);
    }

    #[test]
    fn drives_progress_blocks_with_fraction() {
        // Two blocks: one mid-run (continue), one final (end).
        let stream = "\
frame=10\n\
out_time_us=1000000\n\
total_size=2048\n\
speed=2.0x\n\
progress=continue\n\
frame=20\n\
out_time_us=2000000\n\
total_size=4096\n\
speed=1.5x\n\
progress=end\n";

        let mut got = Vec::new();
        drive_progress(Cursor::new(stream), Some(4.0), |p| got.push(p));

        assert_eq!(got.len(), 2);
        assert_eq!(got[0].out_time_secs, 1.0);
        assert_eq!(got[0].total_size, Some(2048));
        assert_eq!(got[0].speed, Some(2.0));
        assert_eq!(got[0].fraction, Some(0.25)); // 1s of a 4s input

        assert_eq!(got[1].out_time_secs, 2.0);
        assert_eq!(got[1].fraction, Some(0.5));
    }

    #[test]
    fn fraction_is_none_without_duration() {
        let stream = "out_time_us=1000000\nprogress=end\n";
        let mut got = Vec::new();
        drive_progress(Cursor::new(stream), None, |p| got.push(p));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].fraction, None);
    }

    #[test]
    fn tail_keeps_last_nonempty_lines() {
        let lines: Vec<String> = ["a", "", "b", "c", ""].iter().map(|s| s.to_string()).collect();
        assert_eq!(tail(&lines, 2), "b\nc");
    }

    #[test]
    fn tokenize_keeps_quoted_spaces_together() {
        assert_eq!(
            tokenize(r#"-i "my movie.mkv" -map 0:a:1"#),
            vec!["-i", "my movie.mkv", "-map", "0:a:1"]
        );
    }

    #[test]
    fn tokenize_treats_backslashes_as_literal() {
        // A Windows path must survive unescaped and unsplit when quoted.
        assert_eq!(
            tokenize(r#"-i "C:\Media\The Film.mkv""#),
            vec!["-i", r"C:\Media\The Film.mkv"]
        );
        // Unquoted backslash path stays intact too.
        assert_eq!(tokenize(r"-i C:\a\b.mkv"), vec!["-i", r"C:\a\b.mkv"]);
    }

    #[test]
    fn tokenize_handles_single_quotes_and_empties() {
        assert_eq!(tokenize("-metadata title='Le Film'"), vec!["-metadata", "title=Le Film"]);
        assert_eq!(tokenize(r#"-metadata title="""#), vec!["-metadata", "title="]);
        assert_eq!(tokenize("   "), Vec::<String>::new());
    }
}
