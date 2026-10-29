# interlace

A focused native desktop app for **remuxing** media -- reordering, tagging,
removing, extracting, inserting, and converting the streams inside a video file --
built as a friendly GUI over `ffmpeg`/`ffprobe`.

Five of its six operations are lossless container edits (`-c copy`): they rewrap
your streams in seconds without re-encoding. Only audio conversion re-encodes.

## What it does

- **Reorder** streams by dragging rows into the output order you want;
- **Tag** language, title, and `default`/`forced` flags per stream -- plus a
  title for the whole output file. Select several rows (Ctrl/Shift-click) to
  edit them all at once;
- **Remove** streams you don't want (and Restore them if you change your mind);
- **Extract** a single stream to its own file (`.aac`, `.srt`, …), picking the
  right container automatically;
- **Insert** an external audio or subtitle track from another file;
- **Convert** audio to AAC/AC3/Opus/FLAC/MP3 with bitrate and channel
  control, in place or as an extra stream next to the original.

### Checking sync

- **Sync offset** on any audio or subtitle track, whether it came from another
  file or lives inside the video: nudge it in 0.05 s steps until it lines up.
- **Subtitle cues**: list a text subtitle's lines with their times (offset
  included), filter them, and click one to jump the preview there.
- **Preview clips**: render 10, 30 or 60 seconds of the real output from any
  start time and open it in your default player. The track you're testing is
  the only one of its kind in the clip, so the player can't pick another.

Together that's one quick loop: read the lines, click one, preview, nudge the
offset, preview again.

### Drag and drop

Drop files anywhere on the window. An overlay shows exactly what will happen
before you let go: **open** a video, **replace** the current one (you're asked
first if you have edits), or **insert** audio and subtitle files as tracks.
Files it can't use are listed as skipped. Drops, and removing a source file, can
be undone from the toast that follows, or with **Ctrl+Z**.

### The command

The assembled ffmpeg command is one click away (the terminal button), and
**editable** -- tweak it and hit Run to execute your own command verbatim.
Before a run, interlace flags common container/codec mismatches (e.g. an ASS
subtitle bound for an MP4) with a suggestion. While it runs you get progress,
elapsed time and speed, and a Cancel button; if ffmpeg fails, **View log** shows
its full error output.

## Requirements

interlace calls `ffmpeg` and `ffprobe` -- it does **not** bundle them.

- Install a recent ffmpeg build and put `ffmpeg`/`ffprobe` on your `PATH`, or
- Open **Settings** (the gear button) and point it at your binaries directly.

Settings shows the detected version (or a clear error) for each. If ffmpeg
can't be found, a banner says so and links straight to Settings.

## Usage

1. Launch `interlace.exe`, or run `interlace <file>`, or drop a video on the
   window (or use **Add file…**).
2. Insert extra tracks with **+ Audio** / **+ Subtitle**, or drop them in.
3. Edit: drag rows to reorder, click a row to edit it in the inspector on the
   right, use **Remove** / **Extract to file…** at its bottom.
4. Check the output path in the bottom bar, then press **Run**. If the output
   exists you'll be asked before it's overwritten.

Wide windows (1400 px and up) split the table into codec, language, title and
source columns, and add a stream-details section to the inspector. The
inspector and the command panel can both be resized by dragging their edges.

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
- Bundled fonts: Ubuntu (Ubuntu Font Licence, see `assets/fonts/UFL.txt`) and
  Phosphor icons (MIT, via `egui-phosphor`). Subtitle text in other scripts uses
  the Windows system fonts (Segoe UI, Microsoft YaHei, Yu Gothic, Malgun Gothic,
  Leelawadee UI) when present.
