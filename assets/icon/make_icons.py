#!/usr/bin/env python3
"""Build the raster app icons from gifforge.svg.

The SVG adapts to the KDE colour scheme. Its default colours are the mono
dark look, which is what every other platform gets as PNG, ICO and ICNS.

    pip install cairosvg pillow
    python3 assets/icon/make_icons.py            # write the icon files
    python3 assets/icon/make_icons.py --preview  # also render KDE previews
"""
import io
import re
import sys
from pathlib import Path

import cairosvg
from PIL import Image

HERE = Path(__file__).resolve().parent
SVG = (HERE / "gifforge.svg").read_text()


def render(svg: str, size: int) -> Image.Image:
    png = cairosvg.svg2png(bytestring=svg.encode(), output_width=size, output_height=size)
    return Image.open(io.BytesIO(png)).convert("RGBA")


def recolour(bg: str, text: str, highlight: str, hl_text: str) -> str:
    """What KDE's icon loader does: swap the colour scheme stylesheet."""
    style = (
        f".ColorScheme-Background {{ color: {bg}; }}"
        f".ColorScheme-Text {{ color: {text}; }}"
        f".ColorScheme-Highlight {{ color: {highlight}; }}"
        f".ColorScheme-HighlightedText {{ color: {hl_text}; }}"
    )
    return re.sub(r'(<style id="current-color-scheme"[^>]*>).*?(</style>)', lambda m: m.group(1) + style + m.group(2), SVG, flags=re.S)


def main() -> None:
    sizes = [16, 24, 32, 48, 64, 128, 256, 512]
    imgs = {s: render(SVG, s) for s in sizes}
    for s in (16, 32, 48, 128, 256, 512):
        imgs[s].save(HERE / f"gifforge-{s}.png")
    imgs[256].save(HERE / "gifforge.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
    imgs[512].save(HERE / "gifforge.icns")
    print("wrote PNG, ICO and ICNS")

    if "--preview" in sys.argv:
        schemes = [
            ("default", None),
            ("breeze-dark", ("#202326", "#fcfcfc", "#3daee9", "#ffffff")),
            ("breeze-light", ("#eff0f1", "#232629", "#3daee9", "#ffffff")),
            ("dark-pink", ("#202326", "#fcfcfc", "#e93d82", "#ffffff")),
            ("light-orange", ("#eff0f1", "#232629", "#f67400", "#ffffff")),
        ]
        sheet = Image.new("RGB", (len(schemes) * 220, 300), (128, 128, 128))
        for i, (name, cols) in enumerate(schemes):
            svg = SVG if cols is None else recolour(*cols)
            panel = (27, 30, 34) if name in ("default", "breeze-dark", "dark-pink") else (239, 240, 241)
            sheet.paste(Image.new("RGB", (220, 300), panel), (i * 220, 0))
            big = render(svg, 176)
            sheet.paste(big, (i * 220 + 22, 20), big)
            for j, s in enumerate((48, 32, 16)):
                im = render(svg, s)
                sheet.paste(im, (i * 220 + 30 + j * 64, 220), im)
        out = HERE.parent.parent / "target" / "icon-preview.png"
        out.parent.mkdir(exist_ok=True)
        sheet.save(out)
        print(f"preview: {out}")


if __name__ == "__main__":
    main()
