# Interlace

A focused native desktop app for **remuxing** media -- reordering, tagging,
removing, extracting, inserting, and converting the streams inside a video file --
built as a friendly GUI over `ffmpeg` / `ffprobe`.

Five of its six operations are lossless container edits (`-c copy`): they rewrap
your streams in seconds without re-encoding. Only audio conversion re-encodes.

![Interlace](assets/icon.png)

## What it does

- **Reorder** streams by dragging rows into the output order you want;
- **Tag** language, title, and `default`/`forced` flags per stream -- plus a
  title for the whole output file;
- **Remove** streams you don't want;
- **Extract** a single stream to its own file (`.aac`, `.srt`, …), picking the
  right container automatically;
- **Insert** an external audio or subtitle track from another file, with a
  per-track sync offset to line it up against the video;
- **Convert** audio to AAC/AC3/Opus/FLAC/MP3 with bitrate and channel
  control.

The assembled ffmpeg command is always visible at the bottom, and **editable** --
tweak it and hit Run to execute your own command verbatim. Before a run, Interlace
flags common container/codec mismatches (e.g. an ASS subtitle bound for an MP4)
with a suggestion.

## Requirements

Interlace calls `ffmpeg` and `ffprobe` -- it does **not** bundle them.

- Install a recent ffmpeg build and put `ffmpeg` / `ffprobe` on your `PATH`, or
- Open the **⚙** panel in the app and point it at your binaries directly.

The **⚙** panel shows the detected version (or a clear error) for each.

## Usage

1. Launch `interlace.exe`, or drop a file on it, or run `interlace <file>`.
2. Add a file with **+ add file** (or start from the one you opened).
3. Edit: drag to reorder, click a row to edit it in the inspector, use the
   row actions to remove or convert.
4. Check the command bar, then press **▶ Run**. If the output exists you'll be
   asked before it's overwritten.

### Command-line

The logic core is also scriptable:

```
interlace <file> --print            # print the ffmpeg command and exit
interlace <file> --run [--out P]    # run it, streaming progress, and exit
```

## Building from source

Requires a recent Rust toolchain (edition 2024).

```
cargo run                    # launch the GUI
cargo test                   # run the test suite
scripts\package.ps1          # build a release .zip in dist\
```

## Notes

- Windows-first; the code is portable but only packaged/tested on Windows so far.
