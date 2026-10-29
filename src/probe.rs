//! Building a `Project` from a real file by shelling out to `ffprobe` and
//! parsing its JSON.
//!
//! The serde structs here mirror ffprobe's output shape exactly (`Raw*`), kept
//! separate from the domain model so the JSON schema and our model can evolve
//! independently. We map each probed stream onto an `OutStream` set to
//! `Encode::Copy` — the default project keeps every source stream, in source
//! order.
//!
//! Probing (`probe`) and building the model (`Project::from_probe`,
//! `Project::append_probed`) are separate steps, so the UI can run the slow
//! one, the ffprobe call, on a worker thread and apply its result afterwards.

use crate::model::{Encode, Input, Kind, Meta, OutStream, Project, Source};
use crate::run::no_window;
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

// --- serde structs mirroring `ffprobe -print_format json` --------------------

/// One file's ffprobe report, for `Project::from_probe` or
/// `Project::append_probed`. Its contents stay private to this module.
#[derive(Debug, Deserialize)]
pub struct ProbeOutput {
    #[serde(default)]
    streams: Vec<RawStream>,
    format: Option<RawFormat>,
}

#[derive(Debug, Deserialize)]
struct RawStream {
    index: usize,
    codec_type: String,
    #[serde(default)]
    codec_name: String,
    /// Bits per second as a decimal string, e.g. "128000". Absent for some
    /// streams (notably lossless/variable). Parsed to kbps in `map_probe_streams`.
    #[serde(default)]
    bit_rate: Option<String>,
    #[serde(default)]
    tags: RawTags,
    #[serde(default)]
    disposition: RawDisposition,
}

#[derive(Debug, Default, Deserialize)]
struct RawTags {
    language: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawDisposition {
    // ffprobe reports these as 0/1 integers.
    #[serde(default)]
    default: u8,
    #[serde(default)]
    forced: u8,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    // A string like "1234.567000" in seconds; parsed lazily below.
    duration: Option<String>,
    // Container-level tags (`title`, ...). We only read `title` today.
    #[serde(default)]
    tags: RawTags,
}

// --- invocation --------------------------------------------------------------

/// The error a cancelled probe returns.
pub const CANCELLED: &str = "cancelled";

/// Stops probes from another thread: `cancel` kills the ffprobe running under
/// this token and makes every later `probe_cancellable` with it fail at once.
/// Clones share the same token, one for the worker and one for the canceller.
#[derive(Clone, Default)]
pub struct ProbeCancel(Arc<Mutex<CancelSlot>>);

/// What `ProbeCancel` shares between the two threads.
#[derive(Default)]
struct CancelSlot {
    /// Set by `cancel`; checked before each spawn, so none starts after it.
    cancelled: bool,
    /// The ffprobe running now, shared with the thread waiting on it.
    child: Option<Arc<Mutex<Child>>>,
}

impl ProbeCancel {
    /// Kill the running ffprobe, if any, and refuse to start another.
    pub fn cancel(&self) {
        let mut slot = lock(&self.0);
        slot.cancelled = true;
        if let Some(child) = &slot.child {
            let _ = lock(child).kill();
        }
    }
}

/// `mutex`'s guard, poisoned or not: the data here stays valid after a panic.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `ffprobe` on `file` and deserialize its JSON. `ffprobe` is the program
/// to invoke: a bare `"ffprobe"` resolves via PATH, else the ⚙ panel's path.
/// It blocks until ffprobe exits.
pub fn probe(ffprobe: &str, file: &Path) -> Result<ProbeOutput, String> {
    probe_cancellable(ffprobe, file, &ProbeCancel::default())
}

/// `probe`, stoppable through `cancel` from another thread, for the UI's
/// worker. A cancelled probe returns `Err(CANCELLED)`. `no_window` matters
/// here: without it a release build (GUI subsystem) opens a console window for
/// the call, which takes focus and flashes the app.
pub fn probe_cancellable(ffprobe: &str, file: &Path, cancel: &ProbeCancel) -> Result<ProbeOutput, String> {
    let mut cmd = Command::new(ffprobe);
    no_window(&mut cmd);
    cmd.args([
        "-v",
        "error", // stay quiet on success, but send real errors to stderr
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ])
    .arg(file)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    // Spawn while holding the slot, so a cancel can't land between the check
    // and the spawn and miss the new process.
    let (child, mut stdout, stderr) = {
        let mut slot = lock(&cancel.0);
        if slot.cancelled {
            return Err(CANCELLED.into());
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not launch ffprobe (`{ffprobe}`): {e}"))?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let child = Arc::new(Mutex::new(child));
        slot.child = Some(Arc::clone(&child));
        (child, stdout, stderr)
    };

    // Read both pipes without holding the child's lock, so `cancel` can kill
    // it meanwhile; stderr gets its own thread so neither pipe can fill up.
    let stderr = std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let mut json = Vec::new();
    let read = stdout.read_to_end(&mut json);
    // Both pipes are closed now, so the process is ending and this is brief.
    let status = lock(&child).wait();
    let stderr = stderr.join().unwrap_or_default();
    let cancelled = {
        let mut slot = lock(&cancel.0);
        slot.child = None;
        slot.cancelled
    };

    if cancelled {
        return Err(CANCELLED.into());
    }
    let status = status.map_err(|e| format!("could not wait on ffprobe: {e}"))?;
    read.map_err(|e| format!("could not read ffprobe's output for {}: {e}", file.display()))?;
    if !status.success() {
        return Err(format!("ffprobe failed for {}: {}", file.display(), stderr.trim()));
    }

    serde_json::from_slice(&json)
        .map_err(|e| format!("could not parse ffprobe JSON for {}: {e}", file.display()))
}

/// Map ffprobe's stream list onto `OutStream`s referencing input index `input`,
/// each copy-encoded and skipping types we don't model. Shared by `from_input`
/// (the primary, `input = 0`) and `append_input` (an embedded track). Callers set
/// `added` themselves — probed streams are part of the file, appended ones aren't.
fn map_probe_streams(streams: &[RawStream], input: usize) -> Vec<OutStream> {
    let mut out = Vec::new();
    for rs in streams {
        // Skip stream types we don't model rather than failing the probe.
        let Some(kind) = Kind::from_codec_type(&rs.codec_type) else {
            continue;
        };
        out.push(OutStream::new(
            Source {
                input,
                index: rs.index,
                kind,
                codec: rs.codec_name.clone(),
                // bits/sec → kbps, rounded to nearest; `None` if absent/unparseable.
                bitrate_kbps: rs
                    .bit_rate
                    .as_deref()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|bps| ((bps + 500) / 1000) as u32),
            },
            Meta {
                language: rs.tags.language.clone(),
                title: rs.tags.title.clone(),
                default: rs.disposition.default != 0,
                forced: rs.disposition.forced != 0,
            },
            Encode::Copy,
        ));
    }
    out
}

impl Project {
    /// Build the initial project from probing `file`: every source stream kept,
    /// in source order, each copy-encoded, output path defaulting to
    /// `<name>.remux.mkv` next to the input.
    pub fn from_input(ffprobe: &str, file: &Path) -> Result<Project, String> {
        Ok(Project::from_probe(file, probe(ffprobe, file)?))
    }

    /// Build the initial project for `file` from its `probed` report: the
    /// second half of `from_input`, for callers that probed on another thread.
    pub fn from_probe(file: &Path, probed: ProbeOutput) -> Project {
        let streams = map_probe_streams(&probed.streams, 0);

        let duration_secs = probed
            .format
            .as_ref()
            .and_then(|f| f.duration.as_deref())
            .and_then(|d| d.parse::<f64>().ok());

        // Seed the container title from the source so an existing title shows in
        // the editor (and round-trips through the copy) rather than looking unset.
        let title = probed.format.as_ref().and_then(|f| f.tags.title.clone());

        Project {
            inputs: vec![Input::new(file.to_path_buf())],
            streams,
            title,
            output: default_output_path(file),
            duration_secs,
        }
    }

    /// Embed the single audio or subtitle track of `file`, as its `probed`
    /// report describes it, as a new input,
    /// placed after the last stream of its kind (or at the end when there is
    /// none), returning its index (for the UI to select).
    ///
    /// The embed model is one external track, not a container merge: we take the
    /// lone audio/subtitle stream and ignore any incidental video (cover art) or
    /// attachment/data. Errors — leaving the project untouched — if the file has no
    /// such track, or more than one (ambiguous), so we never emit an orphan `-i`.
    /// `output` and `duration_secs` are left as they are (an embedded track that
    /// runs longer than the primary just clamps the progress bar at 100%).
    pub fn append_probed(&mut self, file: &Path, probed: ProbeOutput) -> Result<usize, String> {
        let idx = self.inputs.len();
        let mut embeddable: Vec<OutStream> = map_probe_streams(&probed.streams, idx)
            .into_iter()
            .filter(|s| matches!(s.source.kind, Kind::Audio | Kind::Subtitle))
            .collect();

        let name = file.display();
        let mut stream = match embeddable.len() {
            0 => return Err(format!("{name} has no audio or subtitle track to embed.")),
            1 => embeddable.pop().unwrap(),
            _ => {
                return Err(format!(
                    "{name} has multiple tracks — embed a file with a single audio or subtitle track."
                ));
            }
        };
        stream.added = true; // synthetic — hard-deleted, not soft-removed

        // After the last stream of the same kind, so a new subtitle joins the
        // subtitles rather than landing after the attachments.
        let kind = stream.source.kind;
        let at = self
            .streams
            .iter()
            .rposition(|s| s.source.kind == kind)
            .map_or(self.streams.len(), |i| i + 1);
        self.inputs.push(Input::new(file.to_path_buf()));
        self.streams.insert(at, stream);
        Ok(at)
    }
}

/// `movie.mkv` -> `movie.remux.mkv` (in the same directory).
fn default_output_path(file: &Path) -> PathBuf {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    let parent = file.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem}.remux.mkv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(index: usize, codec_type: &str, codec_name: &str) -> RawStream {
        RawStream {
            index,
            codec_type: codec_type.into(),
            codec_name: codec_name.into(),
            bit_rate: None,
            tags: RawTags::default(),
            disposition: RawDisposition::default(),
        }
    }

    #[test]
    fn map_probe_streams_stamps_input_and_keeps_probed() {
        let raws = vec![
            raw(0, "video", "h264"),
            raw(1, "audio", "aac"),
            raw(2, "banana", "n/a"), // unmodeled type is skipped, not fatal
        ];
        let streams = map_probe_streams(&raws, 3);

        assert_eq!(streams.len(), 2); // the unmodeled stream dropped
        assert!(streams.iter().all(|s| s.source.input == 3));
        assert!(streams.iter().all(|s| !s.added)); // probed, not synthesized
        // Absolute ffprobe indices are preserved (the audio keeps index 1).
        assert_eq!(streams[1].source.index, 1);
        assert_eq!(streams[1].source.kind, Kind::Audio);
    }

    #[test]
    fn map_probe_streams_parses_source_bitrate_to_kbps() {
        let mut audio = raw(0, "audio", "aac");
        audio.bit_rate = Some("137599".into()); // bits/sec → 138 kbps (rounded)
        let no_rate = raw(1, "audio", "flac"); // absent → None

        let streams = map_probe_streams(&[audio, no_rate], 0);
        assert_eq!(streams[0].source.bitrate_kbps, Some(138));
        assert_eq!(streams[1].source.bitrate_kbps, None);
    }

    /// A cancelled token starts nothing: even a binary that doesn't exist
    /// reports the cancel, not a launch failure.
    #[test]
    fn cancelled_probe_never_spawns() {
        let cancel = ProbeCancel::default();
        cancel.cancel();
        let result = probe_cancellable("interlace-test-no-such-ffprobe", Path::new("movie.mkv"), &cancel);
        assert_eq!(result.unwrap_err(), CANCELLED);
    }
}
