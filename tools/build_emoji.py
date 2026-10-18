#!/usr/bin/env python3
"""Bundle the emoji set: Twemoji SVGs plus emojibase names, keywords and groups.

    npm pack emojibase-data && tar xzf emojibase-data-*.tgz       # -> package/
    git clone --depth 1 --filter=blob:none --sparse https://github.com/jdecked/twemoji
    (cd twemoji && git sparse-checkout set assets/svg)
    python3 tools/build_emoji.py package twemoji/assets/svg

Writes assets/emoji/twemoji.zip (one SVG per emoji) and assets/emoji/emoji.json.
"""
import json
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "emoji"

# emojibase group -> our category (the "component" group is left out)
GROUPS = {0: 0, 1: 1, 3: 2, 4: 3, 5: 4, 6: 5, 7: 6, 8: 7, 9: 8}


def main(pkg: str, svg_dir: str) -> None:
    data = json.load(open(Path(pkg) / "en" / "data.json"))
    svgs = {p.stem: p for p in Path(svg_dir).glob("*.svg")}

    def file_for(hexcode: str):
        h = hexcode.lower()
        if h in svgs:
            return h
        h2 = "-".join(x for x in h.split("-") if x != "fe0f")
        return h2 if h2 in svgs else None

    used = {}
    out = []
    missing = 0
    for e in sorted(data, key=lambda e: e.get("order", 1 << 30)):
        g = e.get("group")
        if g not in GROUPS:
            continue
        f = file_for(e["hexcode"])
        if not f:
            missing += 1
            continue
        used[f] = svgs[f]
        skins = []
        for s in e.get("skins", []):
            if not isinstance(s.get("tone"), int):
                continue  # mixed tones (couples etc.) are left out
            sf = file_for(s["hexcode"])
            if sf:
                used[sf] = svgs[sf]
                skins.append([s["tone"], s["emoji"], sf])
        tags = " ".join(sorted(set(e.get("tags", []) + e.get("shortcodes", []))))
        out.append({"e": e["emoji"], "f": f, "g": GROUPS[g], "n": e["label"], "k": tags, "s": [[x[1], x[2]] for x in sorted(skins)]})

    OUT.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(OUT / "twemoji.zip", "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for name, p in sorted(used.items()):
            z.writestr(f"{name}.svg", p.read_bytes())
    (OUT / "emoji.json").write_text(json.dumps(out, ensure_ascii=False, separators=(",", ":")))
    tones = sum(len(x["s"]) for x in out)
    print(f"{len(out)} emoji, {tones} skin-tone versions, {len(used)} SVGs, {missing} without a graphic")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
