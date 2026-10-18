#!/usr/bin/env python3
"""Bundle a selection of openly licensed fonts (OFL, Apache 2.0, Ubuntu Font
Licence) from a checkout of github.com/google/fonts.

    git clone --depth 1 --filter=blob:none --sparse https://github.com/google/fonts.git
    (cd fonts && git sparse-checkout set --no-cone 'ofl/oswald/*' ...)
    pip install fonttools
    python3 tools/build_fonts.py path/to/fonts

Writes assets/fonts/fonts.zip (the font files), assets/fonts/fonts.json (families,
groups and styles) and assets/FONTS_LICENSE.txt.
"""
import json
import sys
import zipfile
from pathlib import Path

from fontTools.ttLib import TTFont

# directory, files to leave out
FONTS = [
    ("ofl/oswald", []), ("ofl/anton", []), ("ofl/bebasneue", []), ("ofl/archivoblack", []), ("ofl/russoone", []),
    ("ofl/roboto", []), ("ofl/opensans", []), ("ofl/montserrat", ["Montserrat-Italic"]), ("ofl/rubik", []),
    ("ofl/ptsansnarrow", []), ("ufl/ubuntu", ["Ubuntu-Light", "Ubuntu-MediumItalic", "Ubuntu-LightItalic"]),
    ("ofl/didactgothic", []), ("ofl/jura", []), ("ofl/fredoka", []), ("ofl/comicneue", ["ComicNeue-Light"]),
    ("ofl/ebgaramond", []), ("ofl/playfairdisplay", ["PlayfairDisplay-Italic"]), ("ofl/poiretone", []),
    ("ofl/lobster", []), ("ofl/pacifico", []), ("ofl/marckscript", []), ("ofl/caveat", []), ("apache/permanentmarker", []),
    ("ofl/gloriahallelujah", []), ("ofl/amaticsc", []), ("ofl/shantellsans", ["ShantellSans-Italic"]),
    ("ofl/bangers", []), ("apache/luckiestguy", []), ("ofl/lilitaone", []), ("ofl/righteous", []), ("ofl/shrikhand", []),
    ("ofl/coiny", []), ("ofl/oi", []), ("ofl/ruslandisplay", []), ("ofl/rubikglitch", []), ("ofl/rubikvinyl", []),
    ("ofl/rubikmonoone", []), ("ofl/creepster", []), ("ofl/pressstart2p", []), ("ofl/vt323", []),
    ("ofl/amiri", ["Amiri-Italic", "Amiri-BoldItalic"]), ("ofl/cairo", []),
    ("ofl/arimo", []), ("ofl/frankruhllibre", []),
    ("ofl/kanit", ["Italic", "Kanit-Thin", "Kanit-ExtraLight", "Kanit-Medium", "Kanit-SemiBold", "Kanit-ExtraBold"]),
    ("ofl/catamaran", []), ("ofl/mallanna", []), ("ofl/mandali", []), ("ofl/timmana", []),
    ("ofl/hind", ["Hind-Light", "Hind-Medium", "Hind-SemiBold"]), ("ofl/notosansgeorgian", []),
    ("ofl/dohyeon", []), ("ofl/blackhansans", []), ("ofl/zcoolkuaile", []), ("ofl/delagothicone", []),
]

# fonts made mainly for another script; the rest are grouped by whether they have Cyrillic
SCRIPT_OF = {
    "delagothicone": "Japanese", "dohyeon": "Korean", "blackhansans": "Korean", "zcoolkuaile": "Chinese (Simplified)",
    "amiri": "Arabic", "cairo": "Arabic", "arimo": "Hebrew", "frankruhllibre": "Hebrew", "kanit": "Thai", "catamaran": "Tamil",
    "mallanna": "Telugu", "mandali": "Telugu", "timmana": "Telugu", "hind": "Devanagari", "notosansgeorgian": "Georgian",
}
GROUP_ORDER = ["Latin & Cyrillic", "Latin", "Arabic", "Hebrew", "Thai", "Tamil", "Telugu", "Devanagari", "Georgian", "Japanese", "Korean", "Chinese (Simplified)"]
WEIGHTS = {100: "Thin", 200: "ExtraLight", 300: "Light", 400: "Regular", 500: "Medium", 600: "SemiBold", 700: "Bold", 800: "ExtraBold", 900: "Black"}


def name(f, ids):
    for i in ids:
        n = f["name"].getDebugName(i)
        if n:
            return n
    return None


def group_of(f, dirname):
    if dirname in SCRIPT_OF:
        return SCRIPT_OF[dirname]
    return "Latin & Cyrillic" if 0x0416 in f.getBestCmap() else "Latin"


def styles_of(f):
    """(style name, [(axis, value)]) for every style the file offers."""
    if "fvar" in f:
        fvar = f["fvar"]
        defaults = {a.axisTag: a.defaultValue for a in fvar.axes}
        out = []
        for inst in fvar.instances:
            n = f["name"].getDebugName(inst.subfamilyNameID) or "Regular"
            coords = [[t, v] for t, v in inst.coordinates.items() if v != defaults.get(t)]
            # keep only the weight and italic/slant axes: the extra axes of a few fonts are odd
            if any(t not in ("wght", "ital", "slnt") for t, _ in coords):
                continue
            out.append((n, coords))
        return out
    return [(name(f, [17, 2]) or "Regular", [])]


def main():
    src = Path(sys.argv[1])
    here = Path(__file__).resolve().parent.parent / "assets"
    fams = {}
    licences = []
    zf = zipfile.ZipFile(here / "fonts" / "fonts.zip", "w", zipfile.ZIP_DEFLATED, compresslevel=9)
    for d, skip in FONTS:
        dirp = src / d
        files = sorted(p for p in dirp.glob("*.ttf") if not any(s in p.name for s in skip))
        if not files:
            print("missing", d)
            continue
        for p in files:
            f = TTFont(p)
            family = name(f, [16, 1])
            entry = fams.setdefault(family, {"family": family, "group": group_of(f, dirp.name), "faces": []})
            arc = f"{dirp.name}/{p.name}"
            for style, coords in styles_of(f):
                if any(x["style"] == style for x in entry["faces"]):
                    continue
                entry["faces"].append({"style": style, "file": arc, "coords": coords})
            zf.write(p, arc)
        for lic in ("OFL.txt", "LICENSE.txt", "UFL.txt"):
            if (dirp / lic).exists():
                licences.append(f"==== {dirp.name} ====\n\n" + (dirp / lic).read_text(errors="replace").strip() + "\n")
                break
    zf.close()
    out = sorted(fams.values(), key=lambda e: (GROUP_ORDER.index(e["group"]), [x[0] for x in FONTS].index(next(d for d, _ in FONTS if d.endswith("/" + e["faces"][0]["file"].split("/")[0])))))
    # Regular first, then by weight, italics last
    def key(fc):
        s = fc["style"].lower()
        w = next((k for k, v in WEIGHTS.items() if v.lower() in s.replace(" italic", "").replace("italic", "regular" if s == "italic" else "")), 400)
        return ("italic" in s, s != "regular", w)
    for e in out:
        e["faces"].sort(key=key)
    (here / "fonts" / "fonts.json").write_text(json.dumps(out, indent=1))
    (here / "FONTS_LICENSE.txt").write_text(
        "Fonts bundled with GIFkit. Each is free software under the licence below:\n"
        "the SIL Open Font License 1.1, the Apache License 2.0 or the Ubuntu Font Licence 1.0.\n"
        "They come from github.com/google/fonts.\n\n" + "\n\n".join(licences)
    )
    print(len(out), "families", sum(len(e["faces"]) for e in out), "styles", (here / "fonts" / "fonts.zip").stat().st_size // 1024, "KiB")
    for e in out:
        print(f'{e["group"]:22} {e["family"]:24} {", ".join(x["style"] for x in e["faces"])}')


if __name__ == "__main__":
    main()
