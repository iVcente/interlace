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
  - **Sync offsets are per stream** (`OutStream::offset_secs`), but `-itsoffset` shifts a whole input. `InputPlan` handles this: each model input keeps its own `-i` at its own index (input 0 stays first, with offset 0, because ffmpeg takes chapters and global metadata from it). Each other distinct (file, offset) pair then gets an extra `-i` of the same file with that offset, and maps point at it. An added single-track file just carries its track's offset on its own `-i`.
  - `preview_args` is the same command cut to a window. Each `-i` is seeked to `start - offset` in place of `-itsoffset`; near 0 the seek clamps and the leftover goes back into `-itsoffset`. An output-side `-ss 0` is required: under stream copy, input seeks are coarse and `.srt` demuxers don't seek at all, so without it earlier cues arrive with negative timestamps and the muxer shifts the whole clip. This was verified against real ffmpeg, so don't drop it. Video is re-encoded in previews (`PREVIEW_VIDEO`: x264 ultrafast, ≤720p). A copied video can only start at a keyframe, and players freeze on the first frame until it arrives.
  - It never emits `-y`/`-n`. The overwrite decision belongs to the run layer, after it checks whether the output exists. The UI asks the user through a confirm modal driven by `pending_run`.
- `extract.rs`: builds a one-stream project. Getting the output extension right is enough to produce a correct command, because `args.rs` already handles elementary outputs. It falls back to `.mka`/`.mks`/`.mkv` when a codec has no clean elementary muxer.
- `validate.rs`: rule-based hints, not a gate. They cover container/codec mismatches such as ASS→MP4, attachments outside Matroska, and AAC in WebM. The checks use the stream's *effective* codec: the conversion target if it is re-encoded, otherwise the source codec.
- `run.rs`: spawns ffmpeg with `-progress pipe:1`. One thread parses the `key=value` blocks from stdout into `RunUpdate`s over an `mpsc` channel. Another thread drains stderr to explain failures. It deliberately uses plain threads and no async runtime. `run_raw` runs a user-edited argv verbatim. On Windows, `no_window` stops a console window from appearing.
- `subs.rs`: reads one text subtitle stream's cues for the cue viewer. ffmpeg converts the stream to SRT on stdout (`-c:s srt -f srt -`) and a lenient parser reads that, so only one format is parsed. Image-based subtitles (PGS/VobSub/DVB) are rejected up front by `is_text_subtitle`.
- `preview.rs`: preview-clip helpers. `focused` makes the stream under test the *only* stream of its kind in the clip. Players pick tracks by their own rules, like the first track or a preferred language (MPC-HC), and an untagged added `.srt` loses either way. The stream under test is the selected audio/subtitle stream, or else one with an offset. It also provides `parse_timestamp` and temp paths. Each clip gets a fresh name, since players lock open files. The app deletes its own clips in `Drop` and sweeps leftovers at startup. The UI runs the clip through `run::run_preview` and the shared `run_state`, and `RunState::Running::open_on_success` opens it in the default app.
- `main.rs`: with no flags it launches the UI. `--print`/`--run` take the headless CLI path over the same core.

UI (`src/ui/`): `InterlaceApp` in `ui/mod.rs` owns all session state. Each screen region is a submodule (`sources`, `table`, `inspector`, `cues`, `command`, `runbar`) and receives `&mut InterlaceApp` to draw itself. `poll_run` drains the run channel every frame and calls `request_repaint` while a job is active.

- Look: the "Midnight / floating cards" design. Every color is a named token in `ui/theme.rs`; never spell a raw `Color32` elsewhere. `theme::install` embeds Ubuntu Regular/Medium (`assets/fonts/`, a `medium` font family) and Phosphor icons (`egui-phosphor` 0.13 = egui 0.35), plus Windows CJK/Arabic/Thai system fonts as fallbacks. Tests must build their context with `theme::install(&ctx, false)` or the `medium` family panics. Shared pieces (cards, buttons, badges, pills, checkbox, offset stepper) live in `ui/widgets.rs`.
- Layout: panels are transparent with the 8 px gutter as margin and draw their own card. Windows ≥1400 px get the wide form (split table columns, 440 px inspector, STREAM DETAILS); the inspector panel uses a different id per mode so each keeps its default. Table rows and cue rows use explicit per-index ids, not auto ids, or layout changes flash id-clash outlines. Debug builds also flag a widget whose rect stays put while its id changes (`warn_if_rect_changes_id`); auto ids count the widgets drawn earlier in the parent, and even an `id_salt` scope mixes that count in, so a region whose earlier content varies (the inspector) needs `UiBuilder::id` scopes. The `*_does_not_flash` tests guard this.
- Run and Preview share one slot, `run_state` (`Idle | Running { handle, job } | Done { job, outcome }`). `RunHandle::cancel` kills ffmpeg (Run and Preview turn into Cancel). Only a real run locks the inspector.
- Drops (`ui/drop.rs`): `DropPlan` classifies hovered files by extension into exactly one action (open, replace, insert N, replace-then-insert, refused) and the overlay shows it. Replacing a file with edits goes through `pending_drop`. Drops and source-chip removals push a whole-session snapshot onto `undo` (`ui/undo.rs`) with a toast; Ctrl+Z pops it. Inserted tracks go after the last stream of their kind (`append_input`).
- Selection: `app.selection` (`ui/selection.rs`) is a set of row indices plus a Shift-click anchor. The inspector edits every selected stream at once and shows a "mixed" state where they differ. Extract, view cues and sync offset need exactly one stream (`selection.single()`). Sync offset is offered for any audio or subtitle track, not video. Any mutation that shifts `streams` must fix up the selection (`on_removed`, `set_single`, `set_many`).
- Command-bar escape hatch: while `command_edit` is `None`, the bar mirrors `to_args()` live. Once the user edits it, it becomes `Some(text)`. Run then tokenizes that text (`run::tokenize`), drops a leading `ffmpeg` token (the configured binary takes precedence) and runs it verbatim. Validation hints are hidden in this state. Loading a file or pressing Reset sets it back to `None`.
- The trait method passes the root `Ui` to `render(&mut self, ui)`, which is kept separate so tests can drive it. eframe 0.35's `App::ui` takes a root `Ui`, and panels are shown *into* a `Ui`.
- Cue card (`ui/cues.rs`): stacked under the inspector in the right column, which then splits 50/50. Its OFFSET stepper edits the same `offset_secs` as the inspector. Parsed cues are cached for the session in `cue_cache`. A preview tests the selected track, else the cue card's (`preview_subject`). It's opened from the inspector and stays on that stream rather than following the selection, because loading reads the whole file. `app.cues` is keyed by input *path* plus source stream index (rows and input indices shift), loads on a thread (`cues::poll` runs every frame), and is dropped once its stream leaves the project. Shown times add the stream's live `offset_secs`. Clicking a cue writes `app.preview_start` (2 s lead-in).
- When the command panel is toggled, the code calls `request_discard` on purpose (`prev_show_command`). This prevents a one-frame flash of id-clash outlines. The check runs after the sources card is drawn, because its terminal button flips `show_command` mid-frame.

## Conventions

- Each module opens with a `//!` doc block that explains its design reasoning. Items carry `///` docs that explain *why*. Keep both up to date when you change behavior.
- `Cargo.lock` is committed because this is a binary crate.
