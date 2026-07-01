//! Building a `Project` from a real file by shelling out to `ffprobe` and
//! parsing its JSON.
//!
//! The serde structs here mirror ffprobe's output shape exactly (`Raw*`), kept
//! separate from the domain model so the JSON schema and our model can evolve
//! independently. We map each probed stream onto an `OutStream` set to
//! `Encode::Copy` — the default project keeps every source stream, in source
//! order.

use crate::model::{Encode, Kind, Meta, OutStream, Project, Source};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

// --- serde structs mirroring `ffprobe -print_format json` --------------------

#[derive(Debug, Deserialize)]
struct ProbeOutput {
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
}

// --- invocation --------------------------------------------------------------

/// Run `ffprobe` on `file` and deserialize its JSON. `ffprobe` is the program
/// to invoke — a bare `"ffprobe"` resolves via PATH (a configurable override
/// comes later).
fn probe(ffprobe: &str, file: &Path) -> Result<ProbeOutput, String> {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error", // stay quiet on success, but send real errors to stderr
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(file)
        .output()
        .map_err(|e| format!("could not launch ffprobe (`{ffprobe}`): {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffprobe failed for {}: {}",
            file.display(),
            stderr.trim()
        ));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("could not parse ffprobe JSON for {}: {e}", file.display()))
}

impl Project {
    /// Build the initial project from probing `file`: every source stream kept,
    /// in source order, each copy-encoded, output path defaulting to
    /// `<name>.remux.mkv` next to the input.
    pub fn from_input(ffprobe: &str, file: &Path) -> Result<Project, String> {
        let probed = probe(ffprobe, file)?;

        let mut streams = Vec::new();
        for rs in &probed.streams {
            // Skip stream types we don't model rather than failing the probe.
            let Some(kind) = Kind::from_codec_type(&rs.codec_type) else {
                continue;
            };
            streams.push(OutStream {
                source: Source {
                    input: 0,
                    index: rs.index,
                    kind,
                    codec: rs.codec_name.clone(),
                },
                meta: Meta {
                    language: rs.tags.language.clone(),
                    title: rs.tags.title.clone(),
                    default: rs.disposition.default != 0,
                    forced: rs.disposition.forced != 0,
                },
                encode: Encode::Copy,
            });
        }

        let duration_secs = probed
            .format
            .as_ref()
            .and_then(|f| f.duration.as_deref())
            .and_then(|d| d.parse::<f64>().ok());

        Ok(Project {
            inputs: vec![file.to_path_buf()],
            streams,
            output: default_output_path(file),
            duration_secs,
        })
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
