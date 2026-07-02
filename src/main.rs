//! Interlace — a focused GUI wrapper around ffmpeg/ffprobe for remuxing media.
//!
//! By default this launches the egui/eframe UI (milestone 3+). The logic-core
//! CLI harness from milestones 1–2 is preserved behind flags:
//!   interlace                      launch the UI
//!   interlace <file>               launch the UI, preloaded with <file>
//!   interlace <file> --print       print the ffmpeg command and exit (no UI)
//!   interlace <file> --run [--out] run it, streaming progress, and exit (no UI)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod extract;
mod model;
mod probe;
mod run;
mod ui;
mod validate;

use model::Project;
use run::RunUpdate;
use std::io::Write;
use std::path::PathBuf;

fn main() -> eframe::Result {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // CLI escape hatch: keep the milestone 1–2 behavior for scripting/testing.
    if argv.iter().any(|a| a == "--print" || a == "--run") {
        std::process::exit(cli(&argv));
    }

    // GUI: optionally preload the first non-flag argument as a file.
    let initial = argv.iter().find(|a| !a.starts_with("--")).map(PathBuf::from);

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([780.0, 720.0])
        .with_min_inner_size([560.0, 480.0])
        .with_title("Interlace");
    // Window/taskbar icon. Embedded at compile time; ignored if it won't decode.
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Interlace",
        options,
        Box::new(|cc| {
            let mut app = ui::InterlaceApp::new(cc);
            if let Some(path) = initial {
                app.load_file(path);
            }
            Ok(Box::new(app))
        }),
    )
}

// --- CLI harness (milestones 1–2) --------------------------------------------

/// Returns a process exit code.
fn cli(argv: &[String]) -> i32 {
    let mut file: Option<String> = None;
    let mut out: Option<String> = None;
    let mut do_run = false;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--run" => do_run = true,
            "--print" => {} // print is implicit below
            "--out" => out = it.next().cloned(),
            _ => file = Some(arg.clone()),
        }
    }
    let Some(file) = file else {
        eprintln!("usage: interlace <media-file> [--print] [--run] [--out <path>]");
        return 1;
    };

    let (ffprobe, ffmpeg) = ("ffprobe", "ffmpeg");
    let mut project = match Project::from_input(ffprobe, &PathBuf::from(&file)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
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
    0
}

/// Run the project and render progress to the terminal.
fn execute(ffmpeg: &str, project: &Project) -> i32 {
    let updates = match run::run(ffmpeg, project, true) {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
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
                        0
                    }
                    Err(f) => {
                        let code = f.code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                        eprintln!("ffmpeg failed (exit {code}):");
                        eprintln!("{}", f.stderr_tail);
                        1
                    }
                };
            }
        }
    }
    0
}
