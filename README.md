# GifForge

A GIF editor and optimiser that runs on your own machine. No uploads, no size limits. Linux, Windows and macOS.

![GifForge with the original on the left and the compressed result on the right](docs/img/app.png)

## What it does

- Crop, resize, rotate, cut, change speed, reverse/boomerang
- Effects, text, censor boxes, image overlays and watermarks as layers you can reorder
- Everything is live and non-destructive, with full undo history
- Side-by-side original vs compressed preview
- Export to GIF, WebP, AVIF, APNG, MP4/WebM, frames or sprite sheets (with Godot `SpriteFrames`)
- "Fit under 10 MB" style exports
- Presets you can reuse on other files

## How small does it get?

Same 255-frame GIF, different export settings:

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/img/sizes-dark.png">
  <img alt="File sizes: original 7.6 MiB, lossless GIF 7.6, lossless WebP 6.4, WebP q80 3.9, lossy GIF 3.7, AVIF 1.7" src="docs/img/sizes-light.png">
</picture>

And what that costs in quality (frame 1, zoomed 2×):

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/img/quality-dark.png">
  <img alt="Zoomed crops of the original, lossy GIF, WebP and AVIF" src="docs/img/quality-light.png">
</picture>

Lossless GIF output is pixel-identical to the input. Lossy GIF halves the size with slight grain. WebP and AVIF go further but need a modern browser or app.

## Download

Grab a build from [Releases](../../releases), or build it yourself:

```bash
cargo run --release -- path/to/anim.gif
```

Linux needs `libxkbcommon-dev` and `libgl1-mesa-dev` to build. MP4, WebM and AVIF export need `ffmpeg` on your PATH.

## License

MIT. Bundled fonts: Noto Sans (OFL) and DejaVu.
