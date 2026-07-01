//! Interlace — a focused GUI wrapper around ffmpeg/ffprobe for remuxing media.
//!
//! Milestone 1 is the logic core with no UI: probe a file, build the default
//! project (every source stream kept, in order), and print the ffmpeg command
//! that would remux it. The UI (eframe/egui) comes in later milestones.

mod args;
mod model;
mod probe;

use model::Project;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(file) = std::env::args().nth(1) else {
        eprintln!("usage: interlace <media-file>");
        eprintln!("  probes the file and prints the ffmpeg remux command it would run");
        return ExitCode::FAILURE;
    };

    // PATH-first resolution: a bare name lets the OS find ffprobe on PATH. A
    // configurable override + startup detection with a legible error is a later
    // (M5) concern.
    let ffprobe = "ffprobe";

    match Project::from_input(ffprobe, &PathBuf::from(&file)) {
        Ok(project) => {
            // The whole point of milestone 1: see the generated command.
            println!("ffmpeg {}", project.to_args().join(" "));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
