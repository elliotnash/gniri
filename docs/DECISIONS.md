# Decisions and spike outcomes

Each M0 spike is a yes/no question. This file records what was run, what was
observed, and the design consequence. "Open" means not yet run.

Test setup for all spikes so far: niri 26.04 nested in the GNOME 51 session
(`niri -c <config> -- <command>`), GTK 4.24.1, Adreno X1-85 on turnip.

## Outcomes

| ID | Question | Status | Outcome |
|---|---|---|---|
| S1 | Does the layer-shell shim initialise from a cargo build? | Open | From gjs it works only with `LD_PRELOAD=/usr/lib64/libgtk4-layer-shell.so.0`. The Rust link order is untested until the toolchain is installed. |
| S2 | Does `backdrop-filter` on a child widget give a per-widget, rounded blur region on a layer surface? | **Pass** | See below. |
| S3 | Does the region track animation and hiding? | Open | The region was re-sent after a relayout (panel exclusive zone arriving), which is encouraging, but animation and hide are untested. |
| S4 | Is `xray false` usable for popups? | **Pass (function)** | Works; GPU cost not yet measured. See below. |
| S5 | Do niri blur settings plus tint folding match blur-my-shell? | Open | Needs GNOME reference shots. |
| S6 | Vulkan or GL renderer? | Partial | The default `GskVulkanRenderer` rendered S2 and S4 correctly. GL not yet compared. |
| S7 | Does `output "winit" { scale 1.25 }` apply? | **Pass** | `niri msg -j outputs`: mode 1600x1000, logical 1280x800, scale 1.25. Screenshots are 1600x1000. |
| S8 | Does nested niri render while covered; can it be full-screen sized? | Open | |
| S9 | Do typed keys reach an exclusive top-layer surface in the overview? | Open | Needs `wtype`. |
| S10 | Does the backdrop surface sit behind zoomed-out workspaces? | **Pass** | See below. |
| S11 | Does `OverviewOpenedOrClosed` fire at animation start? | Open | |
| S12 | Stage input region behaviour | Open | Needs `wlrctl`. |
| S13 | GtkPopover fallback | Not needed | S2 passed. |
| S14 | JXL wallpaper load and offscreen blur | Open | |
| S15 | GNOME Eval walker and `ScreenshotArea` | Open | Needs unsafe mode. |
| S16 | Clean start on a private session bus | Open | |
| S17 | niri hot corner with the panel in the corner | Open | Needs `wlrctl`. |

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

## Standing rules

- Nested niri: never `--session`; find the instance through the `NIRI_SOCKET`
  its child process inherits.
- Layer-shell from gjs needs the `LD_PRELOAD` above. Scripts under
  `spikes/gjs/` exist only because they could run before the Rust toolchain
  was installed; later spikes are Rust examples.
