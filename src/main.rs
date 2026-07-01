//! Interlace — a focused GUI wrapper around ffmpeg/ffprobe for remuxing media.
//!
//! Milestones 1–2 are the logic core with no UI: probe a file, build the default
//! project (every source stream kept, in order), print the ffmpeg command, and
//! optionally run it while streaming progress. The UI (eframe/egui) comes later.

mod args;
mod model;
mod probe;
mod run;

use model::Project;
use run::RunUpdate;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    // usage: interlace <media-file> [--run] [--out <path>]
    let mut file: Option<String> = None;
    let mut out: Option<String> = None;
    let mut do_run = false;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--run" => do_run = true,
            "--out" => out = argv.next(),
            _ => file = Some(arg),
        }
    }
    let Some(file) = file else {
        eprintln!("usage: interlace <media-file> [--run] [--out <path>]");
        eprintln!("  (default)    probe the file and print the ffmpeg remux command");
        eprintln!("  --run        also run it, streaming progress");
        eprintln!("  --out <path> override the output path (e.g. to a different container)");
        return ExitCode::FAILURE;
    };

    // PATH-first resolution; a configurable override + startup detection is M5.
    let (ffprobe, ffmpeg) = ("ffprobe", "ffmpeg");

    let mut project = match Project::from_input(ffprobe, &PathBuf::from(&file)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(out) = out {
        project.output = PathBuf::from(out);
    }

    println!("ffmpeg {}", project.to_args().join(" "));

    if do_run {
        println!("→ output: {}", project.output.display());
        return execute(ffmpeg, &project);
    }
    ExitCode::SUCCESS
}

/// Run the project and render progress to the terminal, returning a process
/// exit code that reflects success or failure.
fn execute(ffmpeg: &str, project: &Project) -> ExitCode {
    // No UI yet to confirm overwrite, so allow it for the CLI demo. The run
    // layer still pre-checks when overwrite is false.
    let updates = match run::run(ffmpeg, project, true) {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    for update in updates {
        match update {
            RunUpdate::Progress(p) => {
                let pct = p
                    .fraction
                    .map(|f| format!("{:5.1}%", f * 100.0))
                    .unwrap_or_else(|| "  ?  ".into());
                let speed = p.speed.map(|s| format!("{s:.2}x")).unwrap_or_else(|| "?".into());
                let size = p
                    .total_size
                    .map(|b| format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0)))
                    .unwrap_or_else(|| "?".into());
                // `\r` keeps it on one updating line.
                print!(
                    "\r  {pct}  t={:>7.1}s  speed={speed}  size={size}   ",
                    p.out_time_secs
                );
                let _ = std::io::stdout().flush();
            }
            RunUpdate::Finished { result } => {
                println!();
                return match result {
                    Ok(()) => {
                        println!("done.");
                        ExitCode::SUCCESS
                    }
                    Err(f) => {
                        let code = f.code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                        eprintln!("ffmpeg failed (exit {code}):");
                        eprintln!("{}", f.stderr_tail);
                        ExitCode::FAILURE
                    }
                };
            }
        }
    }
    ExitCode::SUCCESS
}
