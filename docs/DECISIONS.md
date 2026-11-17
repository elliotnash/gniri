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
  A panic is caught per backend (`bus::contain`): uncaught, it ends the
  thread and all seven backends on it without a word. Once, that left the
  brightness slider moving while the backlight stayed where the idle dim
  had left it, and no idle lock, with nothing to say which backend it was.
  A command sent to a backend that has stopped is dropped, not predicted,
  and the first one is reported on stderr; so a backend returns only once
  the shell has gone, even with nothing to do (no backlight).
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

## M4: what the lock screen, the dialogs and the agent taught

Two things were tried live that the plan had left open. Nested niri locks:
`ext-session-lock` works in a nested session, and a virtual keyboard's
keys reach the lock surface. And the real path holds outside fixtures: a locker started
by hand in a nested session checked passwords through the system's PAM and
was unlocked by `loginctl unlock-session`.

### Looks

- **St's `st-mix` is not CSS `color-mix`.** St mixes two colours channel
  by channel and the result is opaque. `color-mix` weighs a translucent
  colour by its alpha and carries the alpha over. blur-my-shell's focus
  ring mixes the accent with `rgba(255,255,255,0.9)`: in GNOME that reads
  (133,181,238) at alpha 0.8, the literal translation to GTK read darker
  and fainter. The theme now gives the white as opaque.
- **A dialog's shade is a formula.** GNOME's lightbox leaves the screen
  half as bright in the middle and 30% of that in the corners, linearly in
  the distance from the centre: a radial gradient of black from alpha 0.5
  to 0.85. blur-my-shell blurs the screen under it and dims that to 0.6,
  which folds into the gradient (0.7 to 0.91). The dialog on top is the
  popup tint over the shaded screen, dimmed once more and by the 3% of
  its own shadow. Over flat grey in the middle of the screen GNOME reads
  (25,25,27), and so does gniri.
- **The pixel St adds to a line is not there at every size.** A line of
  11pt or 9pt text is a pixel taller in St than in Pango; at 15pt bold and
  at 20pt the two agree. `st_line_height` is for the small sizes only.
- **GTK CSS has no `overflow`.** Clipping a child to rounded corners is
  the widget's `overflow` property.
- **A picture brings its own size.** A `GtkPicture` child made the avatar
  as large as the image file wanted, and an expanding one stretched the
  box above it ("`vexpand` travels up", again). The avatar is now what St
  makes it: a box with the picture as its CSS background.
- **GNOME's unlock prompt is a pixel left of centre.** Its layout centres
  a 457px column (the width of the notification list) and the 440px prompt
  inside that, rounding down twice. The same nesting reproduces it.

### Structure

- **The locker is a process, and the session backend its keeper.**
  `gniri lock` prints `locked` once the compositor has confirmed and its
  exit status says how it ended; the backend starts it on logind's `Lock`
  signal, releases its sleep inhibitor on `locked`, and starts a new one if
  it dies, since a session whose locker died stays locked with nothing
  shown. For the same reason the systemd unit has `KillMode=process`:
  restarting gniri must not take the lock screen with it.
- **Leaving the lock.** The only ways out are PAM saying yes, and logind's
  `Unlock` signal. After `unlock` the locker waits for the compositor to
  have heard it (`gdk_display_sync`) before it exits: a locker that
  vanishes first leaves the session locked.
- **logind's `session/auto` is for calls only.** Signals come from the
  session's real object, and a process started by the systemd user manager
  is in no session. The session is looked up once: by `XDG_SESSION_ID`,
  by process, then as the user's graphical session.
- **Password and fingerprint need two PAM services.** A login stack set
  up for fingerprints (Fedora's `with-fingerprint`) waits for the reader
  before it looks at a password. GDM runs two conversations side by side
  for that reason, and so does the lock screen. Until `data/pam/` is
  installed it uses the distribution's `password-auth` and
  `fingerprint-auth` where they exist, then `login`.
- **Still no `unsafe`.** `nonstick` wraps PAM and `gtk4-session-lock` the
  session lock.
- **The layer-shell library must not be asked about a lock surface.**
  `gtk_layer_is_layer_window` answers yes for a session-lock window, and
  `gtk_layer_get_namespace` then reads what is not there and crashes.
  Lock windows carry a style class and the widget dump goes by that.
- **Dialogs have a surface of their own**, not the stage the plan put them
  on. A dialog covers the panel and takes every click, where the stage
  leaves the panel free; and the polkit dialog lives in another process.
- **The polkit agent's proxies are written by hand** too (`zbus_polkit`
  was not needed), and the helper's protocol is lines of text: its lines
  are escaped as GLib's `g_strescape` does, answers go back as typed.
  Registering was tried against the real polkit from a nested session: it
  answered that the session already has an agent (GNOME's), so the call is
  understood.
- **A nested session idles on its own clock.** niri counts input to the
  nested window, not to the machine. A nested gniri with real services
  would dim the real backlight and ask logind to lock the real session
  while its user works beside it, so the test configuration turns the
  idle chain off.
- **A scene's lock screen never asks PAM**, and only a debug build takes a
  scene: `gniri lock --fixture` in a release build is refused.

### Harness

- **niri does nothing over IPC for a locked session**: no screenshot, no
  spawn. Pictures of the lock screen come from a preview (the same widgets
  on an ordinary surface); behaviour is checked under the real lock, by
  the widget dump and a virtual keyboard.
- **Helper processes answer for themselves.** `gniri lock` and
  `gniri polkit` serve `dump-tree` and `sync` on sockets of their own, in
  a scene only; a scene step starts one and the harness merges the dumps.
- **A mask must reach the pixel a box ends in.** A box that starts three
  quarters into a pixel reaches a pixel further than its size says. The
  redaction mask missed it, and the scene's avatar showed as a line of
  differing pixels.
- **A key sent to a surface that was just mapped can arrive too early**:
  the compositor moves the keyboard focus a moment later. The checks
  press again, a few times at most, until it shows.
- **Test scripts are code.** A shell `read` without `-r` takes a trailing
  backslash for a line continuation and waits: the stand-in helper hung
  the test that sends it one.
- **No flat capture, no flat tier.** GNOME's lock screen exists only in
  the accidental capture over the real wallpaper; its components are
  compared by layout and by low-passed pixels.

## M5: what the overview taught

### Looks

- **The overview needs no dimming folded in.** A menu's tint is folded
  for niri's undimmed blur (S5). In the overview what is behind gniri's
  widgets is the backdrop surface, which is the wallpaper blurred and
  dimmed to 0.6 already (S14), so blur-my-shell's colours for the dash,
  the entry and the tiles are used as it writes them. Over flat grey the
  dash reads (53,53,55) and the entry (89,89,89), as GNOME's do.
- **Default ("xray") blur in the overview shows what is behind the
  windows there**: the backdrop where no workspace is, a workspace's own
  wallpaper where one is, and never a window. The dash and the entry had
  it at first, as blur-my-shell's dash never shows a window. It read as
  blurring the wallpaper rather than the overview, beside menus that blur
  the real thing, so they now blur what is behind them (`xray false`).
- **What passes behind the entry and the dash is not dimmed.** niri's
  overview puts the workspace above the active one behind the entry, and
  the one below behind most of the dash (it spaces them by a tenth of the
  output's height times the zoom). The backdrop is dimmed already; a
  workspace and its windows are not, and white windows under the
  extension's 7% white entry left white text on white. So while there is
  a workspace above or below, the entry's or the dash's tint has the
  dimming folded in, as a menu's (S5); with none, and over the cover, it
  is the extension's own, which is what GNOME's capture is compared
  with.
- **GNOME's layout is arithmetic, and reproducing the arithmetic
  reproduces the layout.** Where the overview's parts go, how many icons
  a page holds and at what spacing, and how large the dash's icons are,
  are ports of the JavaScript's sums (`overview/layout.rs`). On the
  reference screen's size they give every one of the thirty icons of the
  captured app grid its captured place, to the pixel.
- **St's `offset-y` is a translation**, and so is its port. A negative
  margin made GTK measure the five-pixel dot as minus one pixel high.
- **St centres by rounding down, GTK by rounding up.** A 5px dot in a
  76px box sat a pixel right of GNOME's. The dash's dot has a pixel of
  margin on that side.
- **A size request counts CSS margins in.** The dash's separator, asked
  to be as tall as an icon, came out twelve pixels short: its bottom
  margin was taken from the request. The request is made of a box inside
  it. The same goes for the dash's background, whose height GNOME binds
  to an icon's with a constraint.
- **A widget that is not visible measures as nothing**, so an arrow of
  the app grid that had no page to turn to made the layout count with
  an arrow zero wide. The arrows stay visible and are taken out with
  `set_child_visible`.
- **GNOME Shell bundles the icons of its arrows** (`carousel-arrow-*`);
  they are bundled here too.

### Structure

- **One surface, not three.** The plan had a surface each for the dash,
  the search and the app grid. The app grid has to hide niri's
  workspaces, which means an opaque surface under the entry and the
  dash; and nothing says in what order a compositor stacks the
  surfaces of one layer. So there is one surface per
  output, `gniri-overview`, on the top layer under the panel, with
  GNOME's `ControlsManager` layout inside it. While the windows show, its
  input region is the entry and the dash alone and niri's overview is
  used through the rest of it (the stage's trick, S12).
- **The cover is the backdrop's own texture.** Behind the app grid and a
  search the surface draws the blurred wallpaper again, laid out at the
  output's size and moved up by the panel's height, so that its pixels
  land on the backdrop's. It is the same `GdkTexture`, not a second
  copy. Without a wallpaper of gniri's it is a flat colour.
- **The keyboard is the overview's while it is open** (S9), and the text
  field inside has the focus all along: typing needs no key forwarding.
  What GNOME does with focus (the entry lighting up when typing starts,
  the arrow keys walking through the results) is state here: which
  result is chosen is a number in the overview's state, and the widgets
  show it with a style class. Escape, Enter and the arrows, which niri
  would have handled, are sent back to it as actions when nothing is
  typed.
- **Which applications run is not a service.** The compositor says which
  windows are open; the applications service says what is installed; a
  pure function puts the two together into the dash. Under a fixture the
  windows are real ones, of a test program that takes an application id.
- **Applications are started by niri** (`Action::Spawn`), not by gniri:
  under systemd each then gets a scope of its own, where a child of
  gniri's would live and die in gniri's unit.
- **The search provider backend never waits for one provider.** Each
  question is a task; the loop sleeps on the commands and all the tasks
  at once, and a new search drops the old one's tasks (the rule of M2
  and M3, here with something to lose: a provider is a program the bus
  may first have to start).
- **A scene's search is answered by the scene.** Under a fixture the
  search service is the real model, fed by a few lines that answer any
  search with the scene's results: the same events, in the same order,
  as from the bus.
- **`OnlyShowIn=GNOME` is shown.** GNOME Settings says it is for GNOME
  alone, and gniri offers it in its menus.
- **Favourites live in gniri's config file**, by the rule that it has one
  file of settings and writes one GSettings key. The default is GNOME's
  list.

### Harness

- **Not everything can be compared on a smaller screen**, and what
  cannot is tested as arithmetic instead (docs/TESTING.md, "What depends
  on the size of the screen"). The one setting made for the tests'
  sake is the dash's icon size.
- **niri's own drawing reaches into a crop.** At the production zoom the
  workspace's shadow fell on the dash's margin on the low nested output.
  The test configuration zooms further out.
- **Whose applications?** A capture of the dash and the app grid shows
  what its user has installed. The extract blacks out every icon that is
  not a symbolic one of the theme, and the names under them, and the
  comparison skips those boxes; the scenes bring applications of their
  own.
- **Half pixels, again.** A 6px margin is seven and a half pixels at
  scale 1.25, and the two sides rounded the crop's last row apart. Maps
  use margins that are whole pixels.
- **A `grep` for `niri_socket` also finds `gniri_socket`.**
- **GTK's warnings are in the logs, not in the results.** A run that
  passes can have printed a critical; the logs of a new part are read
  once (`target/visual/logs/*-gniri.log`).

## M6: what the agents, the live settings and the outputs taught

### Looks

- **The network dialog needed no style of its own.** GNOME's puts one
  entry per secret straight into the dialog's message box, which spaces
  them; the classes are the authentication dialog's, which is compared
  with GNOME. The dialog itself was never captured.
- **The pairing dialog is not GNOME Shell's.** GNOME Shell has none; GNOME
  Settings shows gnome-bluetooth's, a window of its own. Its sentences
  were read out of the installed library (`strings` on
  `libgnome-bluetooth-ui-3.0.so`, and the format strings out of its
  translations), since its source is not among the clones. They are in
  the shell's prompt dialog, with the PIN as one large label.

### Structure

- **An agent answers only its service.** Whoever gets an answer from the
  secret agent gets a password, and any program on the system bus can
  call it. So a caller's connection is compared with the one that owns
  NetworkManager's name, as libnm's agent does, and likewise for BlueZ.
  (The polkit agent's answer holds no secret: the helper tells polkit.)
- **Joining a locked network is the request for an open one.**
  NetworkManager makes the profile and asks back for what it lacks. The
  network backend and the agent never speak to each other.
- **The two agents are in the shell**, as GNOME's network agent is, not
  processes of their own like the polkit agent: they share the system
  bus's thread and connection with the other backends.
- **Nothing is kept in the keyring.** NetworkManager stores a Wi-Fi
  password itself unless the connection leaves it to the agent, which is
  rare. GNOME's agent then uses the keyring; talking to the Secret
  Service is a piece of work of its own, and was left out. Likewise VPN
  secrets, which GNOME gets from programs the VPN plugins bring.
- **An agent, and no search for devices.** The plan asked for the agent.
  GNOME's Bluetooth menu lists what is set up and leaves finding new
  devices to GNOME Settings, and gniri's does the same: a menu that
  listed every device in range would look unlike GNOME's each time it
  opened. Offering pairing from the menu is an open choice, not a thing
  that was tried and failed.
- **In a nested session without a fixture the agents register with the
  real services**, as the polkit agent tries to. NetworkManager takes
  several agents; BlueZ asks the one that last asked to be its default.
  So while such a session runs, a question about the real machine's
  Bluetooth can come up in the nested window. It is also the way to try
  both dialogs for real without leaving the session around it.
- **Settings are state like any other**: a store. A widget that shows a
  setting subscribes to it (the clock's format, the battery's number). A
  surface that was built from a setting is built anew, and only that one:
  `Changes::between` says which. The bar is never among them for a
  setting it shows, because a bar that goes and comes back moves every
  window twice.
- **A backend started with a setting has to be told the next one.** The
  idle timers, locking before sleep and the search providers got a
  command each; night light a handle beside its link, since its commands
  are a plain toggle's. The idle backend has a second thread for that,
  which shares the event queue's handle with the one that reads.
- **Watching one file.** GIO's file monitor with `WATCH_MOVES` follows a
  file that is replaced by renaming another over it, and one that only
  comes to exist later. A link's target is watched as well. The plan had
  the watching in the config crate; it is in the shell, where GIO is.
- **A mistake in the file is told of as an application would**: through
  `GApplication`'s notification, which reaches gniri's own server (or
  whichever serves the session). No road of gniri's own into its
  notification list was built for it.
- **A callback may own a subscription to the store it is subscribed to.**
  Dropping the outer subscription then dropped the inner one from inside
  the store's own list, which was borrowed: a panic, and from a
  destructor an abort. Nothing had dropped a surface while the shell kept
  running until a changed setting did; an unplugged output would have
  been next.
- **A list is taken out of its cell while its members are built or
  dropped.** Both run code of other modules, which may ask for the list.
- **What waits for an output and what does not.** With no output left, a
  question from an agent waits and is asked when one returns; the
  end-session dialog is taken as cancelled, so that nothing counts down
  unseen; a banner's turn is over, or nothing after it would get one.
- **An installed file names what it starts by its full path.** niri and
  systemd start programs with whatever `PATH` they were given, which need
  not be a login shell's. The installer writes the binary's path into the
  fragment's binds and the unit.
- **A licence line and no more.** Every source file names its licence in
  the SPDX form. A copyright line with a name on it is the owner's to
  add.

### Harness

- **The nested compositor's output cannot be unplugged**: `niri msg
  output winit off` does nothing there, and no other compositor with
  layer-shell is installed. `gniri msg debug unplug` enters the shell
  where GTK's report of the change does. It found nothing new, the fault
  it was written to look for having shown itself in the config check
  first; it now keeps that fault from coming back.
- **A dump can be taken before a surface that was mapped again has its
  final layout**, `debug sync` notwithstanding: one run in six had a menu
  entry 168 pixels from where it ended up. A click that does not find its
  widget under the pointer looks again where the widget is.
- **Two calls sent from two threads arrive in either order.** A test that
  needs them in line pauses between them.
- **The niri fragment is validated by a test**, as written and as
  installed, where the plan had it as a rule to remember.

## M7: what the folders, the drags, the languages, the light style and the motion taught

### Looks

- **The compositor cannot blur part of a surface under another part of
  it.** blur-my-shell blurs the app grid behind an open folder; the grid
  and the dialog are on the same surface here. The dialog draws the
  overview's parts again behind itself (`GtkWidgetPaintable`), through a
  GSK blur, dimmed by the extension's brightness and the shade: since
  the blur is linear, the shade's black is one more factor.
- **A widget that is not blurred may be scaled.** The rule of S3 is for
  blurred widgets: an icon that shows it would become a folder shrinks
  as GNOME's does.
- **The light style is the same theme compiled again**, as upstream's
  is; blur-my-shell's light colours mostly swap white for black at the
  same strength (`bms-ink()`), with a few values of its own
  (`bms-pick()`). The dark stylesheet came out byte for byte the same
  after the theme was split in two.

### Structure

- **GNOME's drags never reach the compositor, and neither do gniri's.**
  One `GtkGestureDrag` on the overview's whole surface finds the icon
  under the press, and once the pointer has moved past GTK's drag
  threshold claims the sequence, which ends the icon's own click. Icons
  can then be rebuilt during a drag without losing it, which per-icon
  gestures could not survive.
- **Places are those of the grid as it shows**, with the dragged icon
  already moved, as in GNOME, which moves the icon itself during a drag.
  A drop's arithmetic (`reorder_target`) is a port of
  `_getDropTarget`'s, and is tested as such.
- **The arrangement is one value**, where GNOME has three settings
  (`app-picker-layout`, the app folders, `favorite-apps`). Every change
  writes the grid's whole order out, as `_savePages` does, so that what
  moved stays where it was put and what is new comes after it.
- **Favourites have two homes.** The config file's list stays the user's
  to write; a drag's result is kept beside the list it was made from,
  and a config file given another list is the newer word.
- **State the shell writes has a file of its own** (`state.toml`), so
  that the config file is never written by the shell. One that cannot be
  read is set aside, never overwritten.
- **Texts are looked up by GNOME's English** in GNOME's catalogs, so
  gniri's texts have to be GNOME's character for character, ellipsis and
  dashes included: a test reads every one out of the source and finds it
  in the installed catalogs. Writing it took one fix (the Wi-Fi menu's
  heading has an en dash).
- **Dates are written with strftime formats, which are texts like any
  other**, and with the C library's names for months and weekdays.
- **The lock screen asks the shell for the notifications, and waits.**
  It is a process of its own, and the notifications are the shell's: it
  sends what it already shows, and the shell answers once that is not
  what there is (or after half a minute, and it asks again). Its media
  players it asks itself, with the same backend the shell uses.
- **The keyring after all.** M6 left it out as a piece of work of its
  own; it came to a client of four Secret Service interfaces. Its calls
  run on threads of their own, since unlocking a keyring waits on the
  user and NetworkManager may withdraw the request meanwhile. Secrets go
  over the bus in the "plain" session: the encrypted one is a
  Diffie-Hellman exchange and AES, for a bus only the user's processes
  are on. The tests run a gnome-keyring of their own on the private bus,
  with a login keyring made for them and unlocked from its standard
  input, so that nothing of the user's is touched and nothing asks.

### Motion

- **One easing helper, after Clutter's `ease_property`** (`Eased`): a
  value eased by libadwaita's animation, tied to a widget. libadwaita
  skips an animation to its end while the widget is not mapped and when
  GTK's animations are off, as `Main.overview` and St do, so a scene's
  shell sees only ends and nothing hidden animates.
- **GNOME's scales are a bin's** (`ScaleBin`): what it takes up scaled
  (`ScaleLayout`, the dash's items) or only how it looks (an actor's
  scale, the lock screen's clock and prompt), the child allocated at its
  own size with a transform. Still never for what the compositor blurs:
  the banner keeps its plain slide, and the menus do not grow.
- **What goes is kept until it has gone.** Lists rebuilt from the state
  (the dash, the message list, the workspace dots) keep a leaving item in
  its place while it shrinks away, and only then take it out.
- **Some layouts take a progress**, as GNOME's do: a message group's
  between the stack and the open list, the overview's grid between below
  the screen and its place, the lock screen's stack between the clock's
  height and the prompt's. The last fixed a layout: the stack had always
  measured as tall as the prompt, which on a small screen left the
  notifications no room under the clock.
- **The folder dialog draws its own blur, so it carries it while it
  zooms**: what it draws behind itself is placed by its bounds on screen,
  not in its own coordinates, and stays put while the dialog is scaled.
- **A headless mutter is a host that always draws.** A nested niri draws
  only when its host shows its window; behind the host's lock screen
  every animation stands still. mutter's `--headless --virtual-monitor`
  hosts it instead, for watching motion (not for the pixel comparisons:
  its output scale is not the reference's).

### Harness

- **wlrctl can click but not drag.** The harness has a virtual pointer
  of its own (`pointer.rs`), which also moves to a place rather than by
  an offset.
- **A host that is niri needs the nested window sized.** niri makes a
  new window as wide as a column and as high as the output; the harness
  floats it and gives it the reference size, then grows it by whatever
  the host's border takes.
- **A scene that extends another merges its fixture service by
  service**, one level down, which changed none of the scenes there were.
- **Behind gniri's own lock screen a nested session goes on, slowly**,
  where behind GNOME's it stalls; the harness waits longer then.

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
