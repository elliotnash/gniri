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
| S3 | Does the region track animation and hiding? | **Pass with two rules** | Follows layout and CSS translate; opacity leaves it alone; CSS scale breaks it; an emptied surface is never repainted. See below. |
| S4 | Is `xray false` usable for popups? | **Pass** | Works, and costs nested niri no more CPU than xray blur. GPU cost on battery is for the real session. See below. |
| S5 | Do niri blur settings plus tint folding match blur-my-shell? | **Pass** | Panel colour matches GNOME exactly on flat wallpapers. With `passes 3`, `offset 1.3` the panel over the real wallpaper has no pixel more than 2% from GNOME's. See below. |
| S6 | Vulkan or GL renderer? | **Pass: keep the default** | Vulkan and GL both render the panel and send its blur region; same startup (about 220 ms to first frame), idle CPU zero for both. Vulkan used less memory (104 MB against 124 MB resident, debug build). No pin in the shell; the harness sets `GSK_RENDERER=vulkan` so runs are comparable. |
| S7 | Does `output "winit" { scale 1.25 }` apply? | **Pass** | `niri msg -j outputs`: mode 1600x1000, logical 1280x800, scale 1.25. Screenshots are 1600x1000. |
| S8 | Does nested niri keep working when the host does not show it? | **Covered: pass. Locked: fail, handled** | Covered by a full-screen window it keeps answering, drawing client changes and taking screenshots. Behind the lock screen it stalls completely. See below. |
| S9 | Do typed keys reach an exclusive top-layer surface in the overview? | **Pass** | See below. |
| S10 | Does the backdrop surface sit behind zoomed-out workspaces? | **Pass** | See below. |
| S11 | Does `OverviewOpenedOrClosed` fire at animation start? | **Pass** | The event arrived 9 ms and 11 ms after `open-overview` / `close-overview` were sent (animations off in the test config, so this measures event delivery, not the animation itself). |
| S12 | Stage input region behaviour | **Pass** | See below. |
| S13 | GtkPopover fallback | Not needed | S2 passed. |
| S14 | JXL wallpaper load and offscreen blur | **Pass** | GTK loads the Fedora JXL in about 160 ms and honours its colour profile; the blurred backdrop rendered once with GSK matches GNOME's overview background. See below. |
| S15 | GNOME Eval walker and screenshots | **Pass** | 12 states on three wallpapers plus actor-tree dumps, all checked by eye. See below. |
| S16 | Clean start on a private session bus | **Pass** | Under `dbus-run-session` gniri paints its first frame in about 530 ms (220 ms on the host bus; the difference is the private bus starting xdg-desktop-portal). GTK warns that there is no accessibility bus, so the harness sets `GTK_A11Y=none`. |
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

## S4: real blur behind popups (pass)

Script: `spikes/gjs/s4_s10.js`, config `spikes/gjs/s4_s10.kdl`.

- With `layer-rule { match namespace="^gniri-stage$"; background-effect { xray false } }`
  the stage bubble blurs the app window underneath it (black and white stripes
  become flat grey) and the wallpaper where no window is.
- The panel has no rule, so it keeps niri's default xray blur and shows only
  blurred wallpaper even where a window is behind it. This matches
  blur-my-shell's static panel blur.

Cost, measured as nested niri's CPU time over 5 s with a spinner repainting
the window under the bubble (`interact.js`, commands `spin` and `stage on`):

| | xray false (real content) | xray true (wallpaper only) |
|---|---|---|
| Window repainting, no popup | 32% of one core | 32% |
| Same, blurred popup over it | 38% | 38% |

So blurring real content costs niri no more CPU than xray blur. This says
nothing about GPU load, which nested niri cannot show; that is checked in the
real session on battery once the stage exists (M2). If it is too high there,
remove the rule to fall back to xray.

## S10: overview backdrop (pass)

Same script, after `niri msg action open-overview`.

- The surface with namespace `gniri-backdrop` (`place-within-backdrop true`)
  fills the area behind the zoomed-out workspaces.
- The `gniri-wallpaper` surface zooms with each workspace.
- The top-layer panel stays at full size at the top. Its xray blur shows the
  backdrop surface in the overview, not the workspace wallpaper.
- The overlay-layer stage bubble also stays at full size.

Consequence: the wallpaper + pre-blurred backdrop pair works as planned.

## S3: region tracking (pass, with two rules)

Script: `spikes/gjs/interact.js`, command `anim`.

- Sliding the bubble by changing its top margin every 50 ms produced one
  `set_blur_region` per step, each with the new position, in the same commit
  as the frame.
- CSS `transform: translateY(60px)` on the bubble: the region moved by 60 px
  with it, and the blur shows in the new place.
- Opacity 0.5 on the bubble: no new region is sent and niri keeps blurring at
  full strength behind the half-faded tint. So during a fade the blur does
  not fade; it disappears when the surface unmaps. blur-my-shell fades the
  blur with the menu. GNOME's menu fades are short (about 150 ms), so this is
  a small, recorded difference.
- CSS `transform: scale(0.8)` on the bubble: GTK sent a wrong region (the
  lower right part of the bubble only, 110x91 instead of 240x120) and the
  screenshot shows no blur under most of the bubble.
- Hiding the bubble, which left the stage with nothing to draw, produced **no
  commit at all**: the last frame and its blur region stayed on screen until
  the surface was unmapped. GTK skips rendering when the window's render node
  is empty.

Rule: **never scale a blurred widget.** Animate blurred bubbles with
translation and opacity only. GNOME's menus, banners and OSD do exactly that
(slide and fade), so nothing is lost.

Rule: **a mapped transparent surface must always have something to draw.**
Unmap the stage (and the banner and OSD surfaces) the moment their last
visible child goes away, including at the end of a fade-out. Unmapping is
clean (`attach(nil)`, verified in S12).

## S5: matching blur-my-shell (pass)

Measured on GNOME's flat-wallpaper reference shots, at a panel pixel with no
widget:

| Wallpaper | GNOME panel | Predicted by `0.5 * (30,30,33) + 0.5 * 0.6 * wallpaper` |
|---|---|---|
| `#808080` (128,128,128) | (53,53,55) | (53.4, 53.4, 54.9) |
| `#3584e4` (53,132,228) | (31,54,85) | (30.9, 54.6, 84.9) |

So blur-my-shell's panel is exactly its tint over the backdrop dimmed to 0.6,
with no saturation change. The transparent panel in the overview reads
(77,77,77) over grey: the overview background is the blurred wallpaper times
0.6 with no tint.

gniri's panel with the folded tint `rgba(21, 21, 24, 0.7)` over the same grey
in nested niri reads (53,53,55): identical. `crates/theme/scss/_bms.scss`
holds the folding function, and `data/niri/gniri.kdl` sets niri's blur to
`noise 0` and `saturation 1` so colours pass through unchanged.

### Strength

blur-my-shell's default pipeline is its own dual-Kawase blur, calibrated in
`effects/blur/kawase_sampling.js` to a Gaussian with
`sigma = unscaled_radius * scale / 4`. With the default radius 30 that is
sigma 7.5 logical px (9.375 physical px at scale 1.25). It also adds a dither
of at most one 8-bit level, which we ignore.

`tools/blur-fit/fit.py` checks this against GNOME's `search` shot, whose
lower part is the blurred wallpaper (times 0.6) with the gears crossing it:

- Gaussian fit: the error is lowest at exactly sigma 9.375 physical px
  (RMSE 0.29%; 0.69% at sigma 6, 0.75% at sigma 14).
- niri fit (`spikes/gjs/s5_probe.js` in nested niri, same wallpaper pixels):
  lowest at **`passes 3`, `offset 1.3`** (RMSE 0.31%, no pixel more than 2%
  off). `passes 2` with `offset 3` is nearly as close (0.31%) and cheaper,
  but large offsets on few passes risk artifacts; niri's defaults (3 and 3)
  give 1.5% and are visibly stronger.
- End to end: gniri's panel over the real wallpaper against GNOME's panel in
  the widget-free columns: RMSE 0.22%, no pixel more than 2% off.

The panel strip itself cannot be used for the fit: the top of this wallpaper
is a smooth gradient, so every setting scores the same there.

Fitted at output scale 1.25 only. blur-my-shell scales its radius with the
output scale; whether niri's `offset` does is untested.

The user's blur-my-shell settings are all defaults except that application
blur is on for `com.mitchellh.ghostty`. The niri equivalent is a window rule
in the user's own config, not something gniri does.

## S14: wallpaper and backdrop (pass)

Code: `crates/shell/src/surfaces/wallpaper.rs`.

- `Gdk.Texture` loads the 5120x5120 Fedora JXL in about 160 ms. It is about
  105 MB uncompressed, so gniri renders it once to the output's pixel size
  with `GskRenderer::render_texture` and drops the original. With the real
  wallpaper the debug build sits at 118 MB resident (104 MB with none).
- The file carries a wide-gamut ICC profile. GTK converts it as GNOME does:
  four sample points agree with GNOME's desktop shot to within 0.1%.
  (ImageMagick does not convert unless told to; without `-profile` its
  colours are visibly less saturated.)
- The backdrop is the same texture through a GSK blur node and a 40% black
  layer. GSK's blur radius is twice the Gaussian sigma, so sigma 7.5 logical
  px is radius 15. Against GNOME's overview background over the same
  wallpaper pixels: RMSE 0.37%, no pixel more than 2% off. Over flat
  `#808080` it reads (77,77,77), as GNOME's does.
- A blur node sees transparency beyond its child's edges, which would darken
  the screen edges. Mirrored copies of the image around it stand in for
  blur-my-shell's clamp-to-edge sampling.
- `cargo xtask shot` with the overview open is pixel-identical across three
  runs.

## S8: nested niri when the host does not show it

When the host session locks or blanks, a nested niri stops entirely: no
frames, no IPC replies, and it ignores SIGTERM. A harness run started then
hangs until its timeout, and killing only the wrapper process leaves niri and
gniri running.

The harness therefore:

- refuses to start if `org.gnome.ScreenSaver.GetActive` is true;
- wraps the session in `gnome-session-inhibit --inhibit idle`, so a long
  unattended run cannot let the screen lock halfway;
- runs the session in its own process group and stops the whole group with
  SIGKILL on timeout.

Visual tests need an unlocked session.

Covered is fine. With a focused full-screen window of the host session over
the nested niri window, niri answered IPC in 10 ms, a layer surface mapped by
a client during that time was drawn, and the screenshot showed it. So the
user can keep working in other windows during a run. Another workspace and a
minimised window are untested.

The nested output stays 1280x800 logical. Comparisons with GNOME's 1792x1120
use the same wallpaper pixels and component crops, so a full-screen nested
output is not needed.

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
  it contains notification text, search results and the desktop.
  `tools/gnome-ref/extract.py` cuts the committed per-component references
  out of it (`metrics/` and `shots/` beside `raw/`); the panel is there now,
  other components are added as each milestone needs them.
- **State of the set.** All three passes and the dumps are complete and every
  shot was checked by eye. (The first real-wallpaper pass ran behind the lock
  screen and was redone on 2026-11-14.)
- The shots and dumps from that locked run were kept under
  `raw/lock-accidental/`. They are a usable reference for the lock screen
  clock and the unlock prompt (M4), which the plan had assumed could not be
  captured.
- Stage coordinates are logical (1792x1120 at scale 1.25); screenshots are
  physical pixels.

Lessons kept in the script: it refuses to run while the screen is locked,
re-checks before every shot, and holds an idle inhibitor for the run (menus
open and screenshots succeed over the lock screen, so a locked capture looks
plausible until someone views it); a banner never times out while the user is idle,
so banners are dismissed explicitly; each state first makes sure the overview
is closed; the unsafe-mode shield icon is hidden so the panel matches normal
use; capture runs on the empty last workspace to keep windows out of shots.

## M1: what porting the panel taught

These hold for every widget port that follows.

- **GTK rounds computed lengths differently from St.** St rounds to the
  nearest pixel. GTK rounds paddings, margins and borders down when it
  measures, and minimum sizes up. Upstream's three-decimal em values often
  land just under a pixel (6px is 0.409em, which is 5.9987px), so
  `to_em()` in `crates/theme/scss/_drawing.scss` rounds the em value up, and
  a minimum size that must not grow is written as the largest em value not
  above the pixel size (`$panel_height`).
- **A St button's child covers the button's border.** `PanelMenu.ButtonBox`
  gives its child the full height, over the transparent 3px border that
  fakes a margin. In GTK the child gets negative margins of the border width
  to sit in the same place.
- **St's inset box-shadow fill is a background clipped to the padding box**
  in GTK (`background-clip: padding-box`).
- **Text sits one pixel lower by default.** St measures a line of Adwaita
  Sans 11 as 19px tall and Pango under GTK as 18px; centred in the bar that
  put our glyphs a pixel lower. Labels in the panel get 2px of bottom padding
  to move their centre up one pixel. Calibrated for the default font at
  scale 1.25.
- **Symbolic icon palette.** The "success", "warning" and "error" parts of
  symbolic icons are drawn in St's built-in colours (`#4e9a06`, `#f5793e`,
  `#cc0000`), because GNOME's theme never sets them. libadwaita's are
  brighter, so the theme sets `-gtk-icon-palette`.
- **`all: unset`** on a `GtkButton` removes everything libadwaita styles it
  with, which is what a port of an St widget wants to start from.
- **niri hides top-layer surfaces under fullscreen windows** by itself; the
  shell does nothing for that.
- **A fixture turns GTK animations off**, so a screenshot or a widget dump
  never catches something half way.

## M2: what the menu, the display and the services taught

### Looks

- **St sets Adwaita Sans at a larger optical size than GTK.** The font is
  variable; its optical-size axis tightens the spacing as it grows, and GTK
  leaves it at the minimum (14). GNOME's labels were up to 7px narrower than
  ours. Fitted over twenty labels in the reference dumps, St's widths are
  those of the axis at about 1.5 times the font's pixel size (22 for 11pt).
  The theme sets `font-variation-settings` to that (`st-optical-size()`),
  and every label is now within a pixel, the panel clock included. Why St
  ends up there was not established. `gtk-hint-font-metrics` changed nothing.
- **libadwaita owns some class names.** A widget given the class `dimmed`
  became 55% opaque: libadwaita has a `.dimmed` of its own. Check a new
  class name against libadwaita's stylesheet; ours is `shaded`.
- **A reset must be a bare class selector.** `button.popup-menu-item { all:
  unset }` outranks `.quick-toggle { min-height }` and wipes it. `all:
  unset` goes first in a rule whose selector is a single class.
- **St paints a box-shadow under the element too.** A translucent menu is
  therefore darker than its tint over the blur: the menu over flat grey
  reads the tint over 128 × 0.6 (blur-my-shell's brightness) × 0.8 (the
  shadow's alpha). GTK clips a shadow away under its element, so the factor
  is folded into the menu's colour (`_bms-popup.scss`). The OSD has no
  shadow and reads like the panel.
- **St's shadows reach further.** The menu's 4px shadow fits a Gaussian of
  sigma blur/2 over a box grown by 0.4 × blur; GTK's `st-shadow()` adds that
  as spread.
- **GNOME dims a menu's own pixels under a submenu, not its backdrop**
  (`Clutter.BrightnessContrastEffect` at -0.4 multiplies by 0.6). Here each
  item of the grid gets `filter: brightness(0.6)` and the menu's tint a
  darker colour of the same alpha.
- **A submenu is one flat colour here.** blur-my-shell blurs the submenu as
  a surface of its own, over the menu as it looks before dimming. What is
  under a submenu is only the menu's flat background, so the result is one
  colour over the dimmed menu; `bms-submenu-colour()` derives it.
- **GNOME places a submenu one pixel up and left** of the room its grid
  makes for it (the overlay's offset leaves out the menu's border). Copied.
- **Em lengths keep their fraction when drawn.** GTK lays out with a padding
  rounded down but draws the box with the exact value, so `0.716em` (10.5px)
  gave a button 37px wide in a 36px slot. Lengths St rounds are written as
  the pixel value they round to (`to_em(10px)`, `em_at_most()`).
- **St centres a row in a button and an icon in the row, rounding down
  twice.** A 16px icon beside one 19px line sits a pixel above centre. The
  same nesting with `valign: center` reproduces it.

### Structure

- **Services are state plus two kinds of message** (`gniri_services::Model`):
  events from the backend, commands from the shell, both pure functions on
  the state. A fixture replaces every backend with the prediction function
  and a log, which is what the behaviour checks read.
- **System-bus backends share one connection and one thread.** zbus 6 has no
  blocking API and needs no tokio. It stops reading the socket while any
  signal stream's queue (64 messages) is full: a backend must never await a
  reply while its own signals pile up. NetworkManager and BlueZ therefore
  read all their signals from one ordered stream and make calls beside it.
- **Proxies were written by hand**, not generated with `zbus_xmlgen` as
  planned: each backend uses a handful of properties.
- **Sound runs libpulse's plain main loop on its own thread**, woken for
  commands through a pipe; `libpulse-glib-binding` would have put GLib into
  the services crate.
- **Bluetooth is switched by the kill switch as well as the adapter**, as
  GNOME does through gnome-settings-daemon: `/dev/rfkill` is writable for the
  seat's user (systemd's uaccess rule), so no daemon is needed.
- **Night light uses a Wayland connection of its own** (`wayland-client`),
  never GTK's. niri offers `wlr-gamma-control` only on real outputs: nested,
  the toggle is hidden and the code path cannot run.
- **A custom widget is allowed.** `GniriQuickGrid` subclasses `GtkWidget`
  with `unsafe_code = "forbid"` still in force; the subclassing macros pass.

### Harness

- **Sleeping is not synchronisation.** The first menu screenshot was taken
  before the stage had drawn. `gniri msg debug sync` answers when every
  mapped surface has painted after the request.
- **The host pointer leaks into the nested session.** A toggle was drawn
  hovered because the real pointer was over the nested window. Screenshot
  runs pass `--no-pointer`, which gives every surface an empty input region.
- **`wlrctl` calls are separate clients.** niri may apply two of them in
  either order; the checks pause between them and wait for the target to
  show `hover` before clicking. A surface mapped under a resting pointer is
  not told so until the pointer moves, so the wait nudges it.
- **Half pixels need care.** Two sums that should give the same .5 can
  round apart; the reference crops are cut at whole pixels and say which.
- **Edge antialiasing is not a difference.** See docs/TESTING.md: tier 2
  compares against the range of colours within a pixel.
- **The reference holds private text.** The Wi-Fi name is blacked out of the
  committed pictures and never compared.

## M3: what notifications and the date menu taught

### Looks

- **Translucent cards cannot be painted over one another.** The cards
  under the first of a group of notifications show only as slivers below
  it. St (with blur-my-shell) masks them; simply drawing the first card on
  top would let the others shine through it. `GniriMessageGroup` cuts the
  shape of the card above out of each card below (`push_mask`, inverted
  alpha). The reference pixels confirm it: the top card is the same colour
  over the stack as beside it.
- **A label asks for the width of its text.** A long notification title
  would widen the whole menu. Labels that must take the width they are
  given are ellipsised with `max-width-chars` 1; the theme's `min-width`
  on their container then decides alone.
- **`vexpand` travels up.** An expanding picture inside a message made the
  message itself expand, and the list stretched one card over all its
  spare room. The picture is now its own frame: a `GtkImage` with a
  minimum size, which centres what it shows.
- **A widget's box includes its CSS transform.** A banner starts a little
  above its place and slides down, so for a moment its boxes are 29px up.
  Nothing may be read from them then: the banner's input region comes from
  the surface's size instead, and a check waits for `debug sync` before it
  aims a click.
- **The rules of M1 and M2 held.** With `to_em`, `em_at_most`, the line
  height pixel and the optical size, the date menu and the banner were
  within a pixel of GNOME everywhere on the first run.

### Structure

- **The notification backend keeps a copy of the shell's state** and runs
  the same events and commands through the same pure functions. That copy
  answers what the protocol asks: whether a `replaces_id` still exists,
  and which notifications a change removed, so their senders get
  `NotificationClosed`. The logic exists once.
- **The loop that drains signals never waits for a reply** (see M2). The
  notification server asks for a caller's process id in the method call
  itself; the media player backend is two halves, one that only moves
  signals and commands into a queue and one that works the queue off.
- **Desktop entries are read by hand.** gio 0.22 no longer has
  `DesktopAppInfo` (it moved to a crate of its own), and the services crate
  has no GLib. Name, icon and command are three keys of a text file.
- **One bus runner for both buses** (`Bus::system()`, `Bus::session()`).
- **Which group of notifications is open is menu state**, beside which
  submenu is: Escape, a click elsewhere and the debug interface all reach
  it the same way.
- **The backend stamps notifications with a clock it is handed**, since
  only the shell (GLib) knows the local time and the state must not depend
  on when a test runs.
- **Do Not Disturb lives in memory.** GNOME keeps it in GSettings; the one
  GSettings key gniri writes stays the colour scheme.
- **A banner's surface is anchored to the top and respects the panel's
  exclusive zone**, so the compositor puts it under the panel, wherever
  that ends. The stage covers the output and adds the panel's height
  itself, because it must also catch clicks beside the menu.

### Harness

- **Nobody looked at text that was never drawn.** The notifications under
  the first of a group are in the actor dump with all their text. The
  extract tool replaces the text of fully transparent actors.
- **A banner's check must outlast the banner.** Waiting for one to leave
  takes longer than the three seconds a check normally waits.
- **No capture, no comparison.** GNOME was captured with no media player
  running. The media card is ported from the stylesheet and the source and
  looked at, not measured; it is listed in docs/FIDELITY.md until a capture
  exists.

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
