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
import math
import subprocess
import sys
from pathlib import Path

REFERENCE = Path(__file__).resolve().parents[2] / "tests/visual/reference/gnome-51.1"
RAW = REFERENCE / "raw"
WALLPAPERS = ["flat-808080", "flat-3584e4", "real"]

# name -> what to cut out:
#
#   state      the capture state it comes from
#   match      how to find its root actor: the first actor, depth first,
#              whose fields all match. The shots are cropped to its box.
#   tree       (optional) the same for an ancestor of the root to keep the
#              tree of, for a component whose parts are not all inside its
#              root actor. The quick settings submenus are siblings of the
#              menu, not children.
#   margin     (optional) logical pixels kept around the root's box in the
#              shots, for its shadow. Cut off at the screen edge.
#   keep_text  whether its text is kept. Text is replaced by "x" of the same
#              length unless it is known to be harmless.
#   redact     (optional) actors whose text is private even so: each is
#              {"within": match, "match": match}. Their text is replaced,
#              their box is blacked out in the shots, and they are marked
#              "redacted" so the comparison skips that box.
WIFI_NAME = {
    "within": {"type": "Gjs_status_network_NMWirelessToggle"},
    "match": {"classes": "quick-toggle-subtitle"},
}
QUICK_SETTINGS = {
    "match": {"classes": "popup-menu-content quick-settings bms-popup-blurred"},
    "tree": {"classes": "panel-menu"},
    "margin": 12,
    # Battery level, toggle titles and power profile names.
    "keep_text": True,
    "redact": [WIFI_NAME],
}
EXTRACTS = {
    # Clock and battery percentage only.
    "panel": {"state": "desktop", "match": {"name": "panel"}, "keep_text": True},
    "panel-overview": {"state": "overview", "match": {"name": "panel"}, "keep_text": True},
    "quick-settings": {"state": "quick-settings", **QUICK_SETTINGS},
    "quick-settings-submenu": {"state": "quick-settings-submenu", **QUICK_SETTINGS},
    # No text.
    "osd": {
        "state": "osd",
        "match": {"classes": "osd-window bms-popup-blurred"},
        "margin": 12,
        "keep_text": True,
    },
}


def matches(node, match):
    return all(node.get(key) == value for key, value in match.items())


def find_path(node, match, path=()):
    """The actors from `node` down to the first one matching, or None."""
    path = (*path, node)
    if matches(node, match):
        return path
    for child in node.get("children", []):
        found = find_path(child, match, path)
        if found:
            return found
    return None


def find(node, match):
    path = find_path(node, match)
    return path[-1] if path else None


def redact(node):
    if node.get("text"):
        node["text"] = "x" * len(node["text"])
    for child in node.get("children", []):
        redact(child)


def extract(name, spec):
    dump = json.loads((RAW / "metrics" / f"{spec['state']}.json").read_text())
    path = find_path(dump["tree"], spec["match"])
    if path is None:
        sys.exit(f"{name}: no actor matching {spec['match']} in state {spec['state']}")
    root = path[-1]
    tree = root
    if "tree" in spec:
        ancestors = [actor for actor in path if matches(actor, spec["tree"])]
        if not ancestors:
            sys.exit(f"{name}: the root has no ancestor matching {spec['tree']}")
        tree = ancestors[-1]
    if not spec["keep_text"]:
        redact(tree)

    blackout = []
    for rule in spec.get("redact", []):
        scope = find(tree, rule["within"])
        actor = scope and find(scope, rule["match"])
        if actor is None:
            sys.exit(f"{name}: nothing to redact for {rule}")
        redact(actor)
        actor["redacted"] = True
        blackout.append(actor["box"])

    stage_width, stage_height = dump["stage"]
    source = RAW / WALLPAPERS[0] / f"{spec['state']}.png"
    size = subprocess.run(
        ["magick", "identify", "-format", "%w", str(source)],
        check=True, capture_output=True, text=True).stdout
    ratio = int(size) / stage_width

    # The part of the stage the shots show: the root with its margin, cut off
    # at the screen edge and grown outwards to whole physical pixels, so
    # that the box says exactly which pixels the shots hold.
    x, y, width, height = root["box"]
    margin = spec.get("margin", 0)
    left = math.floor(max(0, x - margin) * ratio)
    top = math.floor(max(0, y - margin) * ratio)
    right = math.ceil(min(stage_width, x + width + margin) * ratio)
    bottom = math.ceil(min(stage_height, y + height + margin) * ratio)
    shot_box = [left / ratio, top / ratio, (right - left) / ratio, (bottom - top) / ratio]

    meta = json.loads((RAW / "meta.json").read_text())
    document = {
        "gnome_shell": meta["gnome_shell"],
        "state": spec["state"],
        # Logical pixels, as are all boxes. Screenshots are physical pixels:
        # multiply by pixels_per_unit.
        "stage": dump["stage"],
        "pixels_per_unit": ratio,
        # The root actor's box, and the part of the stage the shots show.
        "root_box": root["box"],
        "shot_box": shot_box,
        "tree": tree,
    }

    def pixels(value):
        return round(value * ratio)

    for wallpaper in WALLPAPERS:
        source = RAW / wallpaper / f"{spec['state']}.png"
        command = ["magick", str(source), "-crop",
                   f"{right - left}x{bottom - top}+{left}+{top}", "+repage",
                   "-alpha", "off", "-fill", "black"]
        for box_x, box_y, box_width, box_height in blackout:
            command += ["-draw", "rectangle {},{} {},{}".format(
                pixels(box_x) - left - 1, pixels(box_y) - top - 1,
                pixels(box_x + box_width) - left + 1, pixels(box_y + box_height) - top + 1)]
        target = REFERENCE / "shots" / f"{name}-{wallpaper}.png"
        target.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([*command, "-strip", str(target)], check=True)

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
