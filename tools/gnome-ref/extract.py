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
#              length unless it is known to be harmless. Text of an actor
#              that is fully transparent is replaced either way: nobody
#              looked at it to see that it is harmless, and the shots do
#              not show it.
#   redact     (optional) actors whose text is private even so: each is
#              {"within": match, "match": match}. Their text is replaced,
#              their box is blacked out in the shots, and they are marked
#              "redacted" so the comparison skips that box. Every actor
#              that matches inside the first "within" is redacted.
#   raw        (optional) the capture the state is from, a directory under
#              raw/ with its own metrics/ and shots, where that is not the
#              main one.
#   wallpapers (optional) the wallpapers that capture has shots for.
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
# The user's name and picture, wherever they are shown.
USER_PRIVATE = [
    {"within": {"classes": "polkit-dialog-user-layout"},
     "match": {"classes": "user-icon user-avatar"}},
    {"within": {"classes": "polkit-dialog-user-layout"},
     "match": {"classes": "polkit-dialog-user-label"}},
]
LOCK_USER_PRIVATE = [
    {"within": {"classes": "user-widget vertical"},
     "match": {"classes": "user-icon user-avatar"}},
    {"within": {"classes": "user-widget vertical"},
     "match": {"classes": "user-widget-label"}},
]
# GNOME's own lock screen, from the run of the capture that happened behind
# it (docs/DECISIONS.md, S15): over the real wallpaper only.
LOCK = {"raw": "lock-accidental", "wallpapers": ["real"]}
EXTRACTS = {
    # Clock and battery percentage only.
    "panel": {"state": "desktop", "match": {"name": "panel"}, "keep_text": True},
    "panel-overview": {"state": "overview", "match": {"name": "panel"}, "keep_text": True},
    "quick-settings": {"state": "quick-settings", **QUICK_SETTINGS},
    "quick-settings-submenu": {"state": "quick-settings-submenu", **QUICK_SETTINGS},
    # The date, the calendar, the reference notification and an update
    # notice of GNOME Software. The notifications stacked under those two
    # are not drawn, and their text is not kept.
    "date-menu": {
        "state": "date-menu",
        "match": {"classes": "popup-menu-content datemenu-popover bms-popup-blurred"},
        "margin": 12,
        "keep_text": True,
    },
    # The reference notification only.
    "notification-banner": {
        "state": "notification-banner",
        "match": {"classes": "message notification-banner bms-popup-blurred"},
        "margin": 12,
        "keep_text": True,
    },
    # No text.
    "osd": {
        "state": "osd",
        "match": {"classes": "osd-window bms-popup-blurred"},
        "margin": 12,
        "keep_text": True,
    },
    # `pkexec true`: polkit's sentence about it and the dialog's own words.
    "polkit-dialog": {
        "state": "polkit-dialog",
        "match": {"classes": "modal-dialog prompt-dialog bms-popup-blurred"},
        "margin": 24,
        "keep_text": True,
        "redact": USER_PRIVATE,
    },
    # The time and the date.
    "lock-clock": {
        **LOCK,
        "state": "desktop",
        "match": {"classes": "unlock-dialog-clock"},
        "margin": 24,
        "keep_text": True,
    },
    # The prompt's own words. The capture was showing its volume display at
    # the time, well below the prompt.
    "lock-prompt": {
        **LOCK,
        "state": "osd",
        "match": {"classes": "login-dialog-prompt-layout"},
        "keep_text": True,
        "redact": LOCK_USER_PRIVATE,
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


def find_all(node, match):
    """Every actor at or below `node` that matches."""
    found = [node] if matches(node, match) else []
    for child in node.get("children", []):
        found += find_all(child, match)
    return found


def redact(node):
    if node.get("text"):
        node["text"] = "x" * len(node["text"])
    # An inline style can name a file, such as the user's picture.
    node.pop("inline_style", None)
    for child in node.get("children", []):
        redact(child)


def redact_unseen(node):
    """Replaces the text of every actor that is fully transparent."""
    if node.get("opacity") == 0:
        redact(node)
        return
    for child in node.get("children", []):
        redact_unseen(child)


def extract(name, spec):
    raw = RAW / spec["raw"] if "raw" in spec else RAW
    wallpapers = spec.get("wallpapers", WALLPAPERS)

    def shot(wallpaper):
        # A capture of one wallpaper keeps its shots beside its metrics.
        directory = raw / wallpaper if "raw" not in spec else raw
        return directory / f"{spec['state']}.png"

    dump = json.loads((raw / "metrics" / f"{spec['state']}.json").read_text())
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
    redact_unseen(tree)

    blackout = []
    for rule in spec.get("redact", []):
        scope = find(tree, rule["within"])
        actors = find_all(scope, rule["match"]) if scope else []
        if not actors:
            sys.exit(f"{name}: nothing to redact for {rule}")
        for actor in actors:
            redact(actor)
            actor["redacted"] = True
            blackout.append(actor["box"])

    stage_width, stage_height = dump["stage"]
    source = shot(wallpapers[0])
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

    for wallpaper in wallpapers:
        source = shot(wallpaper)
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
    print(f"{name}: {target.relative_to(REFERENCE)} and {len(wallpapers)} shots")


def main():
    names = sys.argv[1:] or list(EXTRACTS)
    for name in names:
        if name not in EXTRACTS:
            sys.exit(f"unknown extract {name!r}; known: {', '.join(EXTRACTS)}")
        extract(name, EXTRACTS[name])


if __name__ == "__main__":
    main()
