#!/usr/bin/env python3
"""Find the niri blur settings that best match blur-my-shell's panel blur.

niri's blur is tuned by `passes` and `offset` in its `blur {}` config section;
blur-my-shell's default is a dual-Kawase blur of radius 30. They are different
kernels, so this searches for the closest pair by comparing gniri's panel over
the real wallpaper with the same strip of a GNOME reference screenshot.

Needs the raw GNOME reference set (tools/gnome-ref/capture.py) and ImageMagick.

Usage: fit.py [--passes 2,3,4] [--offsets 2,3,4,6]
"""

import argparse
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
WORK = REPO / "target/visual/blur-fit"
REFERENCE = REPO / "tests/visual/reference/gnome-51.1/raw/real/notification-banner.png"
WALLPAPER = Path("/usr/share/backgrounds/f45/default/f45-01-night.jxl")

# The nested output is 1600x1000 physical pixels; GNOME's is 2240x1400.
GNOME_SIZE = (2240, 1400)
NESTED_SIZE = (1600, 1000)
PANEL_HEIGHT = 40
# Panel columns with no widgets in the GNOME shot: between the workspace dots
# and the clock, and between the clock and where the nested output ends
# (minus a margin, because blur near an output edge differs).
REGIONS = [(120, 860), (1260, 220)]


def magick(*args):
    return subprocess.run(["magick", *map(str, args)], check=True, capture_output=True, text=True)


def strip(source, destination):
    """Cut the widget-free panel columns out of `source` and join them."""
    parts = []
    for x, width in REGIONS:
        parts += ["(", source, "-crop", f"{width}x{PANEL_HEIGHT}+{x}+0", "+repage", ")"]
    magick(*parts, "+append", destination)


def rmse(a, b):
    result = subprocess.run(
        ["magick", "compare", "-metric", "RMSE", str(a), str(b), "null:"],
        capture_output=True, text=True)
    # Prints "absolute (normalised)" on stderr; exit status 1 just means "differ".
    return float(result.stderr.split("(")[1].rstrip(")\n "))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--passes", default="2,3,4,5")
    parser.add_argument("--offsets", default="2,3,4,6")
    args = parser.parse_args()

    if not REFERENCE.exists():
        sys.exit(f"missing {REFERENCE}: run tools/gnome-ref/capture.py first")
    WORK.mkdir(parents=True, exist_ok=True)

    # The wallpaper as GNOME shows it ("zoom"), cut to the nested output size
    # from the top-left corner so the panel sits over the same pixels.
    wallpaper = WORK / "wallpaper.png"
    side = max(GNOME_SIZE)
    magick(WALLPAPER, "-resize", f"{side}x{side}", "-gravity", "center",
           "-crop", f"{GNOME_SIZE[0]}x{GNOME_SIZE[1]}+0+0", "+repage",
           "-gravity", "northwest",
           "-crop", f"{NESTED_SIZE[0]}x{NESTED_SIZE[1]}+0+0", "+repage", wallpaper)

    reference = WORK / "gnome-strip.png"
    strip(REFERENCE, reference)

    results = []
    for passes in map(int, args.passes.split(",")):
        for offset in map(float, args.offsets.split(",")):
            name = f"blur-fit-p{passes}-o{offset:g}"
            config = WORK / f"{name}.kdl"
            config.write_text(
                f'include "{REPO / "tests/visual/niri/test.kdl"}"\n'
                f"blur {{\n    passes {passes}\n    offset {offset:g}\n"
                "    noise 0\n    saturation 1\n}\n")
            shot = subprocess.run(
                ["cargo", "xtask", "shot", name, "--wallpaper", str(wallpaper),
                 "--niri-config", str(config)],
                cwd=REPO, capture_output=True, text=True)
            if shot.returncode != 0:
                sys.exit(f"screenshot for {name} failed:\n{shot.stderr}")
            ours = WORK / f"{name}-strip.png"
            strip(REPO / "target/visual" / f"{name}.png", ours)
            error = rmse(ours, reference)
            results.append((error, passes, offset))
            print(f"passes={passes} offset={offset:g}  rmse={error:.5f}", flush=True)

    results.sort()
    print("\nbest:")
    for error, passes, offset in results[:5]:
        print(f"  passes={passes} offset={offset:g}  rmse={error:.5f}")


if __name__ == "__main__":
    main()
