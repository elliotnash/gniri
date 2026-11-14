# Decisions and spike outcomes

Each M0 spike is a yes/no question. This file records what was run, what was
observed, and the design consequence. "Open" means not yet run.

Test setup for all spikes so far: niri 26.04 nested in the GNOME 51 session
(`niri -c <config> -- <command>`), GTK 4.24.1, Adreno X1-85 on turnip.

## Outcomes

| ID | Question | Status | Outcome |
|---|---|---|---|
| S1 | Does the layer-shell shim initialise from a cargo build? | **Pass** | `readelf -d target/debug/gniri` lists `libgtk4-layer-shell.so.0` first in NEEDED, and the panel maps as a layer surface with no preload. Keep it that way: never enable gdk4-wayland's `wayland_crate` feature. (gjs still needs `LD_PRELOAD`.) |
| S2 | Does `backdrop-filter` on a child widget give a per-widget, rounded blur region on a layer surface? | **Pass** | See below. |
| S3 | Does the region track animation and hiding? | **Pass with a rule** | Follows layout every frame, but a surface with nothing left to draw is never repainted. See below. |
| S4 | Is `xray false` usable for popups? | **Pass (function)** | Works; GPU cost not yet measured. See below. |
| S5 | Do niri blur settings plus tint folding match blur-my-shell? | Open | Needs GNOME reference shots. |
| S6 | Vulkan or GL renderer? | Partial | The default `GskVulkanRenderer` rendered S2 and S4 correctly. GL not yet compared. |
| S7 | Does `output "winit" { scale 1.25 }` apply? | **Pass** | `niri msg -j outputs`: mode 1600x1000, logical 1280x800, scale 1.25. Screenshots are 1600x1000. |
| S8 | Does nested niri render while covered; can it be full-screen sized? | Open | |
| S9 | Do typed keys reach an exclusive top-layer surface in the overview? | **Pass** | See below. |
| S10 | Does the backdrop surface sit behind zoomed-out workspaces? | **Pass** | See below. |
| S11 | Does `OverviewOpenedOrClosed` fire at animation start? | **Pass** | The event arrived 9 ms and 11 ms after `open-overview` / `close-overview` were sent (animations off in the test config, so this measures event delivery, not the animation itself). |
| S12 | Stage input region behaviour | **Pass** | See below. |
| S13 | GtkPopover fallback | Not needed | S2 passed. |
| S14 | JXL wallpaper load and offscreen blur | Open | |
| S15 | GNOME Eval walker and screenshots | **Pass** | `tools/gnome-ref/capture.py` captured 12 states on 3 wallpapers. See below. |
| S16 | Clean start on a private session bus | Open | |
| S17 | niri hot corner with the panel in the corner | **Pass** | Moving a virtual pointer to 0,0 opens the overview with the top-layer panel covering the corner. No shell-side hot corner needed. |

## S2: blur region from CSS (pass)

Script: `spikes/gjs/s2_blur.js`, config `tests/visual/niri/test.kdl`.

- GTK attaches an `ext_background_effect_surface_v1` to every `wl_surface`
  before our code runs, as predicted. Never bind the protocol ourselves.
- The panel (`backdrop-filter: blur(10px)`, full width) produced
  `set_blur_region` with one rectangle, `0,0 1280x32`.
- The bubble (`blur(10px)`, `border-radius: 36px`) produced a region of 43
  rectangles: one row per pixel through the corners. The screenshot shows a
  clean rounded blur edge with no bleed outside the bubble.
- The second bubble with `blur(4px)` produced no region and is not blurred.
  **Rule: every blurred surface needs `backdrop-filter: blur(10px)` or more.**
  The CSS radius does not change the visible strength; niri's `blur {}`
  settings do.
- The region is in surface-local logical coordinates and follows layout.

Consequence: the full-screen stage design stands. Bubbles are plain widgets
with `backdrop-filter`; no per-bubble surfaces and no niri blur rules needed.

## S4: real blur behind popups (pass on function)

Script: `spikes/gjs/s4_s10.js`, config `spikes/gjs/s4_s10.kdl`.

- With `layer-rule { match namespace="^gniri-stage$"; background-effect { xray false } }`
  the stage bubble blurs the app window underneath it (black and white stripes
  become flat grey) and the wallpaper where no window is.
- The panel has no rule, so it keeps niri's default xray blur and shows only
  blurred wallpaper even where a window is behind it. This matches
  blur-my-shell's static panel blur.

Still to measure: niri CPU/GPU cost of non-xray blur over a repainting window
on this GPU. If it is too high, fall back to xray for popups.

## S10: overview backdrop (pass)

Same script, after `niri msg action open-overview`.

- The surface with namespace `gniri-backdrop` (`place-within-backdrop true`)
  fills the area behind the zoomed-out workspaces.
- The `gniri-wallpaper` surface zooms with each workspace.
- The top-layer panel stays at full size at the top. Its xray blur shows the
  backdrop surface in the overview, not the workspace wallpaper.
- The overlay-layer stage bubble also stays at full size.

Consequence: the wallpaper + pre-blurred backdrop pair works as planned.

## S3: region tracking (pass, with a rule)

Script: `spikes/gjs/interact.js`, command `anim`.

- Sliding the bubble by changing its top margin every 50 ms produced one
  `set_blur_region` per step, each with the new position, in the same commit
  as the frame. Animation by CSS `transform` or `opacity` is not yet tested.
- Hiding the bubble, which left the stage with nothing to draw, produced **no
  commit at all**: the last frame and its blur region stayed on screen until
  the surface was unmapped. GTK skips rendering when the window's render node
  is empty.

Rule: **a mapped transparent surface must always have something to draw.**
Unmap the stage (and the banner and OSD surfaces) the moment their last
visible child goes away, including at the end of a fade-out. Unmapping is
clean (`attach(nil)`, verified in S12).

## S9: typing into search during the overview (pass)

Script: `spikes/gjs/interact.js`, command `search on`, keys sent with `wtype`.

- A top-layer surface with exclusive keyboard interactivity, mapped while the
  overview is open, received every key: letters, BackSpace, arrows, Return and
  Escape.
- niri did not act on any of them: the overview stayed open after Escape and
  the active workspace did not change on arrow keys.

Consequences: type-to-search works as in GNOME. The shell must close the
overview itself on Escape, and niri's own arrow-key navigation inside the
overview is unavailable while the search surface holds the keyboard. Keys
bound in niri's config still take precedence over the surface.

## S12: stage input (pass)

Script: `spikes/gjs/interact.js`, command `stage on`, clicks sent with
`wlrctl` (a full-screen overlay stage that also covers the panel).

- With the stage's input region set to everything below the panel strip, a
  click on the panel reached the panel surface and a click elsewhere reached
  the stage.
- After the stage was unmapped, clicks reached the app window again and it
  kept keyboard focus.

Consequence: the stage sets its input region to exclude the panel while a
panel menu is open, so clicking another panel button switches menus as in
GNOME.

## S15: GNOME reference capture (pass)

`tools/gnome-ref/capture.py` drives the running shell through
`org.gnome.Shell.Eval` and the Screenshot interface (unsafe mode).

- States: desktop, panel-hover, quick-settings, quick-settings-submenu (Power
  Mode), date-menu, overview, app-grid, search, osd, notification-banner,
  run-dialog, polkit-dialog.
- Each state has an actor-tree dump (boxes plus computed theme-node values)
  and a 2240x1400 screenshot on the real wallpaper, flat `#808080` and flat
  `#3584e4`.
- Output is in `tests/visual/reference/gnome-51.1/raw/`, git-ignored because
  it contains notification text, search results and the desktop. Curated
  per-component extracts get committed as each milestone needs them.
- Stage coordinates are logical (1792x1120 at scale 1.25); screenshots are
  physical pixels.

Lessons kept in the script: a banner never times out while the user is idle,
so banners are dismissed explicitly; each state first makes sure the overview
is closed; the unsafe-mode shield icon is hidden so the panel matches normal
use; capture runs on the empty last workspace to keep windows out of shots.

## Standing rules

- Nested niri: never `--session`; find the instance through the `NIRI_SOCKET`
  its child process inherits.
- Layer-shell from gjs needs `LD_PRELOAD=/usr/lib64/libgtk4-layer-shell.so.0`.
  Spikes that only ask how niri or GTK behave are gjs scripts under
  `spikes/gjs/`, since the answer does not depend on the language; anything
  about our own build (S1) is tested with the real binary.
- Scripted input in nested niri: `wtype` for keys, `wlrctl pointer` for
  clicks. `wlrctl pointer move` is relative with no acceleration, so move by
  -3000,-3000 to reach 0,0 first (this trips the hot corner; close the
  overview afterwards). Both need the nested `WAYLAND_DISPLAY`.
