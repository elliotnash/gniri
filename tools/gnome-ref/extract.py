#!/usr/bin/env python3
"""Cut the committed, per-component references out of the raw GNOME capture.

The raw set (tools/gnome-ref/capture.py) is git-ignored: its screenshots and
actor dumps contain notification text, search results and whatever was on the
desktop. This script copies out only what a component's tests need:

  metrics/<name>.json   the component's actor subtree from one state's dump
  shots/<name>-<wallpaper>.png
                        the component's rectangle from that state's screenshots

Add an entry to EXTRACTS when a milestone needs a new component, run this,
look at every new file, and commit them.

Usage: extract.py [name ...]     (default: all)
"""

import json
import subprocess
import sys
from pathlib import Path

REFERENCE = Path(__file__).resolve().parents[2] / "tests/visual/reference/gnome-51.1"
RAW = REFERENCE / "raw"
WALLPAPERS = ["flat-808080", "flat-3584e4", "real"]

# name -> the capture state it comes from, how to find its root actor (the
# first actor, depth first, whose fields all match), and whether its text is
# kept. Text is replaced by "x" of the same length unless it is known to be
# harmless.
EXTRACTS = {
    # Clock and battery percentage only.
    "panel": {"state": "desktop", "match": {"name": "panel"}, "keep_text": True},
    "panel-overview": {"state": "overview", "match": {"name": "panel"}, "keep_text": True},
}


def find(node, match):
    if all(node.get(key) == value for key, value in match.items()):
        return node
    for child in node.get("children", []):
        found = find(child, match)
        if found:
            return found
    return None


def redact(node):
    if node.get("text"):
        node["text"] = "x" * len(node["text"])
    for child in node.get("children", []):
        redact(child)


def extract(name, spec):
    dump = json.loads((RAW / "metrics" / f"{spec['state']}.json").read_text())
    root = find(dump["tree"], spec["match"])
    if root is None:
        sys.exit(f"{name}: no actor matching {spec['match']} in state {spec['state']}")
    if not spec["keep_text"]:
        redact(root)

    meta = json.loads((RAW / "meta.json").read_text())
    document = {
        "gnome_shell": meta["gnome_shell"],
        "state": spec["state"],
        # Logical pixels, as are all boxes. Screenshots are physical pixels:
        # multiply by pixels_per_unit.
        "stage": dump["stage"],
        "pixels_per_unit": None,
        "tree": root,
    }

    x, y, width, height = root["box"]
    for wallpaper in WALLPAPERS:
        source = RAW / wallpaper / f"{spec['state']}.png"
        size = subprocess.run(
            ["magick", "identify", "-format", "%w", str(source)],
            check=True, capture_output=True, text=True).stdout
        ratio = int(size) / dump["stage"][0]
        document["pixels_per_unit"] = ratio
        crop = "{}x{}+{}+{}".format(*(round(value * ratio) for value in (width, height, x, y)))
        target = REFERENCE / "shots" / f"{name}-{wallpaper}.png"
        target.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["magick", str(source), "-crop", crop, "+repage", "-alpha", "off", "-strip",
             str(target)],
            check=True)

    target = REFERENCE / "metrics" / f"{name}.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(document, indent=1) + "\n")
    print(f"{name}: {target.relative_to(REFERENCE)} and {len(WALLPAPERS)} shots")


def main():
    names = sys.argv[1:] or list(EXTRACTS)
    for name in names:
        if name not in EXTRACTS:
            sys.exit(f"unknown extract {name!r}; known: {', '.join(EXTRACTS)}")
        extract(name, EXTRACTS[name])


if __name__ == "__main__":
    main()
