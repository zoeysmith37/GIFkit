# GifForge

A local, cross-platform animated GIF editor and optimiser written in Rust
(egui/eframe). Everything runs on your machine – no uploads, no file size limits.

It covers the same toolset as the popular online GIF editors:

| Tool | What it does |
|---|---|
| **Resize** (live) | Width/height/percentage, *center-crop*, *fit with padding* or *stretch*; Nearest (pixel art), Bilinear, Bicubic, Lanczos3 |
| **Crop** (live) | Drag a box on the preview (move / corner-resize), aspect locks, **auto-trim** to the content of every frame |
| **Rotate** (live) | Flip H/V, 90°/180°/270°, arbitrary angle with canvas expansion and transparent/solid fill |
| **Reverse** (live) | Reverse, boomerang, loop count, seconds counter, flips |
| **Speed** (live) | % of current speed, fixed delay for all frames, or target FPS |
| **Optimize** | Palette size (2–256), global/local palettes, frame differencing, duplicate-frame merging, dithering, **lossy LZW** (gifsicle-style) + temporal lossy, drop every N-th frame, measure & preview the result |
| **Effects** (live) | Hue/saturation/lightness, brightness/contrast, grayscale/sepia/monochrome/negative/tint, background colour, replace colour with transparency (fuzz + edge clean-up), blur/sharpen, ordered & error-diffusion dithering, oil paint/emboss/posterize/solarize/noise, swirl/implode/wave, Gotham/Lomo/Nashville/Toaster/Vignette/Polaroid filters, camera/fuzzy/rounded/border frames, fade in/out |
| **Add text** (live) | Multiple annotations, drag to position, frame ranges, outline, opacity, alignment, load any .ttf/.otf |
| **Censor** (live) | Any number of pixelate / blur / solid regions, each with a frame range |
| **Add image** (live) | Any number of static or animated overlays, per-frame position/size; edits apply to all / following / this frame |
| **Watermark** (live) | Text (any font) or a logo image; pinned to one of 9 anchor points with a margin, or tiled diagonally across the frame; size relative to the frame, opacity |
| **AI shield** (live) | Subtle seeded perturbation (high-frequency pattern, colour noise, optional sub-pixel warp, optional per-frame variation) that makes frames worse AI-training data. Not a guarantee – far weaker than Glaze/Nightshade, and heavy compression removes part of it |
| **Cut** (live) | Keep a range by frame number or by time |
| **Frames** | Thumbnail grid: per-frame delay, skip/enable, duplicate, reorder, delete, range & every-N toggles, crossfade, first-frame background, add frames from files |
| **Export** | One button (Ctrl+S / Ctrl+E): GIF, **animated WebP** (lossy / lossless, built in), **animated AVIF** (via ffmpeg, keeps transparency), APNG, MP4 / WebM (VP9 with alpha) via ffmpeg, frames (PNG/GIF folder or ZIP), **sprite sheet + JSON + Godot 4 `SpriteFrames` (.tres)**. Optional “noai, noimageai” tag in GIF/APNG metadata |

Inputs: animated GIF, APNG, animated WebP, any still image, or a set of images
(dropped together or via *Open sequence…*) which become frames.

## Everything is live

Nothing needs an "Apply" button. Edits are non-destructive and re-render
instantly while the animation plays:

1. **Frame edits** – cut, speed, reverse / boomerang / loop, crop, resize,
   rotate / flip. Each tool has an *Enabled* switch, *Reset* and *Bake frame
   edits*. They sit underneath everything else, so effects and layers are
   applied on top of the cropped / resized / rotated result.
2. **Layers** – effects, text, censor regions, images, watermarks and AI
   shields, in any number and any order. The **Layers** panel on the right
   (Photoshop-style) lists them top → bottom; drag a row to reorder, click the
   eye to hide, hover for ✕ to delete, **+** to add, ⬇ to bake. Long names are
   shortened.

Saving / exporting applies everything. **Bake everything** (or *Bake frame
edits*, which keeps layers live) writes edits into the frames permanently –
undoable like all structural changes.

The Frames tool (skip, delay, reorder, duplicate, delete, crossfade) edits the
source frames directly; all live edits are re-applied on top immediately.

Sliders stop at sensible limits, but you can type values beyond them
(e.g. saturation 400 %).

## Compression

The GIF encoder is lossless unless you ask for lossy settings – every pixel
decodes exactly as it was – and tries hard to be small:

- an exact global colour table from the animation's most common colours;
  frames that fit it don't repeat a local table
- per frame, it trial-encodes several variants and keeps the smallest:
  changed pixels only (rest transparent) vs. the whole changed rectangle,
  global vs. a minimal local table, and a "wildcard" mode where unchanged
  pixels may be written as transparent *or* their own colour, whichever
  continues the current LZW string
- transparent animations: every frame is trimmed to its visible area
- identical consecutive frames are merged

On real files that's 1–14 % smaller than before with identical pixels, and
smaller than the source GIFs we tested. For much smaller files, export
**WebP** (often 25–40 % smaller than the best GIF, even lossless) or **AVIF**
(often 10–30× smaller for screen recordings, modern browsers only).

**Fit under a size** (export window, GIF / WebP / AVIF): give a limit such as
10 MB (Discord) or 256 KB (emotes) and GifForge searches for the best quality
that fits – stronger lossy / fewer colours first, then smaller frames – and
reports the settings it used.

## Undo history

Every change is undoable – frame edits, layer edits, compression settings,
frame operations. ☰ in the toolbar opens the History list (e.g. "Effects:
sepia", "Hide Watermark", "Crop"); click any entry to jump there, later steps
stay available for redo until you make a new change. Slider drags are recorded
as one step once you let go.

## Presets

**Presets** (toolbar) saves the current frame edits, layers and/or compression
settings under a name and re-applies them to any other file. Positions are
scaled to the new frame size; fonts are matched by family; image layers reload
their file. Presets are JSON files in the config folder
(`~/.config/gifforge/presets` on Linux, `%APPDATA%\gifforge\presets` on Windows,
`~/Library/Application Support/gifforge/presets` on macOS).

## Layout

- Left: tools (collapse to icons with «). Right: tool settings on top, layers
  below (the Layers header collapses; the split is resizable).
- Every settings section collapses; ⊟ / ⊞ in the panel header collapse or
  expand them all.
- Most help is in tooltips – hover a control.

## Layers stick to the content

Text, censor regions and images are stored in source-frame coordinates and
carried through every geometric edit underneath them – crop, resize (any
aspect mode), flips, 90° steps, free rotation, and effects layers that move
pixels (flip/rotate effects, polaroid, border). Rotate the GIF and a censor bar
rotates with it, staying over the same thing. Text and images stay readable
after mirroring; censor boxes are mirrored exactly.

Every text, censor and image layer can be rotated: drag the round handle above
its box (Shift snaps to 15°) or type an angle. Corners resize (images keep
aspect unless Shift is held; text corners scale the font size).

## Viewport

- Default view is **split**: left = original file, right = live edit, compressed.
  Click the label in a view's top-left corner to switch it between *Original*,
  *Live edit* and *Compressed*.
- **Each viewport has its own zoom and pan** (⚭ links them).
  The zoom buttons act on the viewport you last used.
- **Mouse wheel** zooms around the cursor; **middle- / right-drag** pans; left-drag
  pans in tools that don't draw on the image (Alt-drag pans anywhere). **F** /
  double-click fits, **1** = 100 %, **+ / −** zoom.
- All editing works in every view whose coordinates line up: live edit,
  compressed, and the original file (for layers).
- Playback keeps running while you edit. Re-compression waits until edits have
  settled (about a second with no changes and no mouse button held). Until it
  finishes, the compressed view shows the live uncompressed preview with an
  orange ⚠ badge, then swaps to the real compressed result.

## Fonts

The font picker (text and watermark layers) lists the built-in fonts plus every
system font family (fontconfig folders on Linux, the system font folders on
Windows/macOS), each **shown in its own typeface**, with search and per-family
styles (weights / italics). *Load font file…* adds any .ttf/.otf/.ttc.

## Performance

Preview frames are pre-rendered in playback order on a background thread pool
that is kept separate from the UI thread, compression previews run on the same
background pool, and playback never "catches up" after a slow frame, so heavy
effect combinations no longer cause stutter or speed bursts.

## Linux / Wayland

winit (the windowing library under egui) doesn't support file drag-and-drop on
native Wayland yet. GifForge therefore runs through **XWayland** on Wayland
sessions by default, where drag-and-drop works. You can switch to native
Wayland in the theme menu (restart needed; or set `GIFFORGE_WAYLAND=1`) – then
use **Ctrl+V** with files copied in Dolphin/Nautilus to open them instead.

## Look

KDE Breeze-style UI (Plasma 6 colour schemes, Noto Sans) with **Breeze Dark**,
**Breeze Light** or **System** – picked from the toolbar and remembered between
runs.

Full undo/redo (Ctrl+Z / Ctrl+Y), drag-and-drop, Space to play/pause, ←/→ to step.

## Build & run

Requires a recent stable Rust toolchain (https://rustup.rs).

```bash
cargo run --release                    # start empty
cargo run --release -- path/to/anim.gif   # open a file
```

The binary ends up in `target/release/gifforge` (`gifforge.exe` on Windows) and is
fully self-contained (fonts are embedded).

**Linux** – needs the usual desktop runtime libraries (X11 or Wayland, OpenGL,
libxkbcommon), which every desktop distro already has. File dialogs use the XDG
desktop portal (KDE/GNOME provide it). On Debian/Ubuntu, building may need
`sudo apt install libxkbcommon-dev libgl1-mesa-dev`.

**Windows / macOS** – `cargo build --release`, nothing else required.

**Video export** (optional) – install `ffmpeg` and make sure it is on your `PATH`.

### Release builds for every platform

`.github/workflows/build.yml` runs the tests and builds on Linux, Windows and
macOS for every push to `main`. Pushing a tag such as `v0.2.0` also creates a
GitHub Release with:

- `gifforge-linux-x86_64.tar.gz`
- `gifforge-windows-x86_64.zip` (no console window)
- `gifforge-macos-universal.zip` – a universal (Apple Silicon + Intel)
  `GifForge.app`, ad-hoc signed. First launch: right-click → Open.

```bash
git tag v0.2.0 && git push origin v0.2.0
```

## Godot workflow

*Export → Sprite sheet*: choose columns/padding, set the
`res://` folder the PNG will live in, export. Drop `name_sheet.png` and
`name_sheet_frames.tres` into that folder and assign the `.tres` to an
`AnimatedSprite2D`/`AnimatedSprite3D`. Variable per-frame delays are preserved
(as SpriteFrames relative durations), and the loop flag follows the GIF loop count.

Tip: for pixel-art sprites use *Resize → Nearest*, tick *Pixelated* in the
preview bar, and *Crop → Auto-trim* to remove empty space before exporting.

## Tests

```bash
cargo test --release
```

Covers GIF round-trips for every encoder mode (including the custom LZW encoder
through dictionary resets), loop counts, APNG, all geometry ops, every effect,
text/censor/overlay rendering and all exporters.

## Notes

- Frames are kept fully composited in memory ("unoptimized"), so every tool works
  per frame and there is never flicker from disposal methods. Optimisation only
  happens at save time.
- Animations with transparency are saved with "don't stack frames" disposal
  automatically.
- GIF delays are in 1/100 s; delays under 2 are shown/played as 10 (browser behaviour).

Fonts: Noto Sans (SIL OFL, `assets/NOTO_OFL.txt`) and DejaVu (`assets/DEJAVU_LICENSE.txt`).
