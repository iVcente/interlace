# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Interlace is a Rust (edition 2024) desktop GUI, built on eframe/egui 0.35, for remuxing media through `ffmpeg`/`ffprobe`. It does not bundle those binaries. It looks them up on `PATH`, or uses paths set in the in-app ⚙ panel. The app is Windows-first: it is packaged and tested only on Windows.

## Commands

```
cargo run                                  # launch the GUI
cargo run -- <file>                        # GUI preloaded with a file
cargo run -- <file> --print                # headless: print the ffmpeg command
cargo run -- <file> --run [--out P]        # headless: run it, stream progress
cargo test                                 # all tests
cargo test <name_substring>                # single test / filter, e.g. cargo test args::tests
scripts\package.ps1                        # release build -> dist\interlace-<ver>-win-x64.zip
```

- No rustfmt or clippy config is checked in, so the defaults apply.
- The test suite does not need ffmpeg installed. Tests build `Project`s by hand and check argv, validation and extract paths. The UI is tested headlessly with `egui::Context::run_ui(..., |ui| app.render(ui))`.
- `profile.dev.package."*"` uses `opt-level = 3` on purpose. Unoptimized egui stutters under `cargo run`.
- `build.rs` embeds `assets/icon.ico` into the `.exe` using `winresource`. If that fails it only prints a warning, and the build still succeeds. The runtime window icon is set separately in `main.rs`.
- `package.ps1` stops any running `interlace.exe`, because a running instance locks the binary. It reads the version from `Cargo.toml`.

## Architecture

The whole app edits one data structure. `model::Project` holds the `inputs` plus a `Vec<OutStream>` kept **in output order**. Every operation is an edit to that vec:
- reorder: reorder the vec
- remove: drop an entry
- convert: set `Encode` on a stream
- insert: add an `Input` and the streams that reference it
- extract: build a separate single-stream `Project` with its own output path

The pipeline:

- `probe.rs`: runs `ffprobe` and parses its JSON into `Raw*` serde structs, which mirror ffprobe's schema exactly. It then maps them into a `Project` (`Project::from_input`). Every stream starts as `Encode::Copy`, in source order.
- `args.rs`: `Project::to_args()` turns the project into an ffmpeg argv, without the program name. Two rules matter:
  - **Output stream specifiers are per type, in `-map` order.** Every `-c`/`-metadata:s`/`-disposition` uses the output index (`:a:1`, ...), not the source index. Keep this per-type counter correct.
  - Every `Meta` keeps the value probed from the source, so the code can compare against it. Clearing a tag that the source had emits `key=`, which deletes it in ffmpeg. Without that, the source value would copy straight through.
  - Elementary outputs (`.aac`, `.srt`, ...) cannot take stream tags or dispositions, so for those it emits only `-map` and `-c copy` (`output_takes_stream_tags`).
  - It never emits `-y`/`-n`. The overwrite decision belongs to the run layer, after it checks whether the output exists. The UI asks the user through a confirm modal driven by `pending_run`.
- `extract.rs`: builds a one-stream project. Getting the output extension right is enough to produce a correct command, because `args.rs` already handles elementary outputs. It falls back to `.mka`/`.mks`/`.mkv` when a codec has no clean elementary muxer.
- `validate.rs`: rule-based hints, not a gate. They cover container/codec mismatches such as ASS→MP4, attachments outside Matroska, and AAC in WebM. The checks use the stream's *effective* codec: the conversion target if it is re-encoded, otherwise the source codec.
- `run.rs`: spawns ffmpeg with `-progress pipe:1`. One thread parses the `key=value` blocks from stdout into `RunUpdate`s over an `mpsc` channel. Another thread drains stderr to explain failures. It deliberately uses plain threads and no async runtime. `run_raw` runs a user-edited argv verbatim. On Windows, `no_window` stops a console window from appearing.
- `main.rs`: with no flags it launches the UI. `--print`/`--run` take the headless CLI path over the same core.

UI (`src/ui/`): `InterlaceApp` in `ui/mod.rs` owns all session state. Each screen region is a submodule (`sources`, `table`, `inspector`, `command`) and receives `&mut InterlaceApp` to draw itself. `poll_run` drains the run channel every frame and calls `request_repaint` while a run is active.

- Command-bar escape hatch: while `command_edit` is `None`, the bar mirrors `to_args()` live. Once the user edits it, it becomes `Some(text)`. Run then tokenizes that text (`run::tokenize`), drops a leading `ffmpeg` token (the configured binary takes precedence) and runs it verbatim. Validation hints are hidden in this state. Loading a file or pressing Reset sets it back to `None`.
- The trait method passes the root `Ui` to `render(&mut self, ui)`, which is kept separate so tests can drive it. eframe 0.35's `App::ui` takes a root `Ui`, and panels are shown *into* a `Ui`.
- When the command panel is toggled, the code calls `request_discard` on purpose (`prev_show_command`). This prevents a one-frame flash of id-clash outlines.

## Conventions

- Each module opens with a `//!` doc block that explains its design reasoning. Items carry `///` docs that explain *why*. Keep both up to date when you change behavior.
- `Cargo.lock` is committed because this is a binary crate.
