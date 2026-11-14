#!/usr/bin/env python3
"""Find the blur settings that best match blur-my-shell's default blur.

blur-my-shell's default pipeline is a dual-Kawase blur calibrated to a
Gaussian of sigma = 30 * scale / 4 physical pixels, times brightness 0.6.
niri's blur is a dual-Kawase tuned by `passes` and `offset`. The panel strip is
too smooth on the default wallpaper to tell settings apart, so the reference
is the overview background in GNOME's `search` shot, which is the blurred
wallpaper with nothing over it across the lower part of the screen.

Two fits:

  gaussian  the Gaussian sigma that reproduces GNOME's blur (ImageMagick only).
            gniri pre-blurs its overview backdrop itself, so it needs this.
  niri      niri's `passes`/`offset`, by screenshotting spikes/gjs/s5_probe.js
            in nested niri. Needs an unlocked screen.

Needs the raw GNOME reference set (tools/gnome-ref/capture.py) and ImageMagick.

Usage: fit.py [gaussian|niri|all] [--passes 2,3,4] [--offsets 1,1.5,2]
"""

import argparse
import os
import signal
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
WORK = REPO / "target/visual/blur-fit"
REFERENCE = REPO / "tests/visual/reference/gnome-51.1/raw/real/search.png"
WALLPAPER = Path("/usr/share/backgrounds/f45/default/f45-01-night.jxl")
PROBE = REPO / "spikes/gjs/s5_probe.js"
LAYER_SHELL = "/usr/lib64/libgtk4-layer-shell.so.0"
SRGB_PROFILE = "/usr/share/color/icc/colord/sRGB.icc"

# Physical pixels. GNOME's screen, and the nested niri output.
GNOME_SIZE = (2240, 1400)
NESTED_SIZE = (1600, 1000)
BRIGHTNESS = 0.6
# The part of GNOME's search shot with only blurred wallpaper: below the
# results and above the dash. The gears cross it, so it has edges to fit.
CLEAN = (0, 800, 2240, 440)  # x, y, width, height
# Where the nested output sits within GNOME's frame, chosen so the clean band
# is far from the nested output's edges (blur near an edge differs).
NESTED_ORIGIN = (320, 400)
EDGE_MARGIN = 150


def magick(*args):
    return subprocess.run(["magick", *map(str, args)], check=True, capture_output=True, text=True)


def crop(source, box, destination, *operations):
    x, y, width, height = box
    magick(source, *operations, "-crop", f"{width}x{height}+{x}+{y}", "+repage",
           "-alpha", "off", destination)


def rmse(a, b):
    result = subprocess.run(
        ["magick", "compare", "-metric", "RMSE", str(a), str(b), "null:"],
        capture_output=True, text=True)
    # Prints "absolute (normalised)" on stderr; exit status 1 just means "differ".
    return float(result.stderr.split("(")[1].rstrip(")\n "))


def gnome_wallpaper():
    """The wallpaper as GNOME shows it ("zoom"): scaled to cover, centred.

    The Fedora wallpaper carries a wide-gamut ICC profile, which GNOME honours;
    without the conversion to sRGB the colours are visibly less saturated.
    """
    path = WORK / "wallpaper-gnome.png"
    side = max(GNOME_SIZE)
    magick(WALLPAPER, "-profile", SRGB_PROFILE,
           "-resize", f"{side}x{side}", "-gravity", "center",
           "-crop", f"{GNOME_SIZE[0]}x{GNOME_SIZE[1]}+0+0", "+repage", "-strip", path)
    return path


def fit_gaussian(wallpaper):
    reference = WORK / "gnome-clean.png"
    crop(REFERENCE, CLEAN, reference)
    results = []
    for sigma in (6, 7, 8, 8.5, 9, 9.375, 10, 10.5, 11, 12, 14):
        ours = WORK / "gaussian.png"
        crop(wallpaper, CLEAN, ours, "-blur", f"0x{sigma}", "-evaluate", "multiply", BRIGHTNESS)
        error = rmse(ours, reference)
        results.append((error, sigma))
        print(f"sigma={sigma:g}  rmse={error:.5f}", flush=True)
    error, sigma = min(results)
    print(f"\nbest Gaussian: sigma={sigma:g} physical px (rmse {error:.5f})\n")


def screen_is_locked():
    result = subprocess.run(
        ["gdbus", "call", "--session", "--dest", "org.gnome.ScreenSaver",
         "--object-path", "/org/gnome/ScreenSaver", "--method", "org.gnome.ScreenSaver.GetActive"],
        capture_output=True, text=True)
    return "true" in result.stdout


def nested_shot(config, wallpaper, destination):
    """Screenshot the probe in a nested niri started with `config`."""
    if screen_is_locked():
        sys.exit("the screen is locked: nested niri cannot render")
    destination.unlink(missing_ok=True)
    inner = (
        f'LD_PRELOAD={LAYER_SHELL} gjs -m "{PROBE}" "{wallpaper}" & '
        "sleep 2; "
        f'niri msg action screenshot-screen --show-pointer false --path "{destination}"; '
        "sleep 0.7; "
        "niri msg action quit --skip-confirmation"
    )
    process = subprocess.Popen(
        ["gnome-session-inhibit", "--inhibit", "idle", "--reason", "gniri blur fit",
         "niri", "-c", str(config), "--", "sh", "-c", inner],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
        env={**os.environ, "GTK_A11Y": "none"})
    try:
        process.wait(timeout=30)
    except subprocess.TimeoutExpired:
        # A stalled nested niri ignores SIGTERM (spike S8).
        os.killpg(process.pid, signal.SIGKILL)
        sys.exit(f"nested niri timed out for {config.name}")
    if not destination.exists():
        sys.exit(f"no screenshot was written for {config.name}")


def fit_niri(wallpaper, passes_list, offsets):
    ox, oy = NESTED_ORIGIN
    nested_wallpaper = WORK / "wallpaper-nested.png"
    crop(wallpaper, (ox, oy, *NESTED_SIZE), nested_wallpaper)

    # The clean band in each frame, kept away from the nested output's sides.
    cx, cy, cw, ch = CLEAN
    left = max(cx, ox + EDGE_MARGIN)
    right = min(cx + cw, ox + NESTED_SIZE[0] - EDGE_MARGIN)
    top = max(cy, oy + EDGE_MARGIN)
    bottom = min(cy + ch, oy + NESTED_SIZE[1] - EDGE_MARGIN)
    gnome_box = (left, top, right - left, bottom - top)
    nested_box = (left - ox, top - oy, right - left, bottom - top)

    reference = WORK / "gnome-band.png"
    crop(REFERENCE, gnome_box, reference)

    results = []
    for passes in passes_list:
        for offset in offsets:
            name = f"p{passes}-o{offset:g}"
            config = WORK / f"{name}.kdl"
            # Later sections override the included ones.
            config.write_text(
                f'include "{REPO / "tests/visual/niri/test.kdl"}"\n'
                f"blur {{\n    passes {passes}\n    offset {offset:g}\n"
                "    noise 0\n    saturation 1\n}\n")
            shot = WORK / f"{name}.png"
            nested_shot(config, nested_wallpaper, shot)
            ours = WORK / f"{name}-band.png"
            crop(shot, nested_box, ours)
            error = rmse(ours, reference)
            results.append((error, passes, offset))
            print(f"passes={passes} offset={offset:g}  rmse={error:.5f}", flush=True)

    results.sort()
    print("\nbest niri settings:")
    for error, passes, offset in results[:5]:
        print(f"  passes={passes} offset={offset:g}  rmse={error:.5f}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("fit", nargs="?", default="all", choices=["gaussian", "niri", "all"])
    parser.add_argument("--passes", default="2,3,4")
    parser.add_argument("--offsets", default="1,1.25,1.5,1.75,2,2.5,3")
    args = parser.parse_args()

    if not REFERENCE.exists():
        sys.exit(f"missing {REFERENCE}: run tools/gnome-ref/capture.py first")
    WORK.mkdir(parents=True, exist_ok=True)
    wallpaper = gnome_wallpaper()

    if args.fit in ("gaussian", "all"):
        fit_gaussian(wallpaper)
    if args.fit in ("niri", "all"):
        fit_niri(wallpaper,
                 [int(p) for p in args.passes.split(",")],
                 [float(o) for o in args.offsets.split(",")])


if __name__ == "__main__":
    main()
