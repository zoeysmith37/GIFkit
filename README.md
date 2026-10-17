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
| **Cut** (live) | Keep a range by frame number or by time |
| **Split** | Frames to PNG/GIF files or a ZIP |
| **Frames** | Thumbnail grid: per-frame delay, skip/enable, duplicate, reorder, delete, range & every-N toggles, crossfade, first-frame background, add frames from files |
| **Save / Export** | GIF, APNG, MP4 / WebM (VP9 with alpha) via ffmpeg, **sprite sheet + JSON + Godot 4 `SpriteFrames` (.tres)** |

Inputs: animated GIF, APNG, animated WebP, any still image, or a set of images
(dropped together or via *Open sequence…*) which become frames.

## Everything is live

Nothing needs an "Apply" button. Edits are non-destructive and re-render
instantly while the animation plays:

1. **Frame edits** – cut, speed, reverse / boomerang / loop, crop, resize,
   rotate / flip. Each tool has an *Enabled* switch, *Reset* and *Bake frame
   edits*. They sit underneath everything else, so effects and layers are
   applied on top of the cropped / resized / rotated result.
2. **Layers** – effects, text, censor regions and images, in any number and any
   order. The *Layer stack* in the sidebar lists them top → bottom; drag the ≡
   handle (or use ▲▼) to reorder – e.g. put a caption above or below a colour
   effect, or one image over another. Each layer has a visibility eye and
   delete button; you can have several effects layers.

Saving / exporting applies everything. **Bake everything** (or *Bake frame
edits*, which keeps layers live) writes edits into the frames permanently –
undoable like all structural changes.

The Frames tool (skip, delay, reorder, duplicate, delete, crossfade) edits the
source frames directly; all live edits are re-applied on top immediately.

Sliders stop at sensible limits, but you can type values beyond them
(e.g. saturation 400 %).

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
  Each side can show the live edit, the original file or the compressed output.
- **Each viewport has its own zoom and pan** (tick *Link views* to tie them).
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

The text font picker lists the built-in fonts plus every system font family
(found via fontconfig on Linux, the system font folders on Windows/macOS), with
search and per-family styles (weights / italics). *Load font file…* adds any
.ttf/.otf/.ttc.

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

`.github/workflows/build.yml` builds Linux, Windows and macOS (Intel + Apple
Silicon) binaries on GitHub Actions when you push a `v*` tag.

## Godot workflow

*Split* or *Save / Export → Sprite sheet*: choose columns/padding, set the
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

## License

Apache-2.0 – see [LICENSE](LICENSE). Bundled fonts keep their own licences (see `assets/`).
