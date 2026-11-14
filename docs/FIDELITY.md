# Where gniri differs from GNOME Shell

The goal is GNOME Shell 51 with blur-my-shell v78 at default settings, dark
style. "Identical" means: theme values equal, layout within half a pixel, and
pixels within the thresholds in `tests/visual/thresholds.toml`. This file
lists everything known to fall outside that, and why. It grows with each
milestone.

## Blur

- **Kernel.** niri's blur and blur-my-shell's are both dual-Kawase but tuned
  differently. With `passes 3`, `offset 1.3` no pixel of the panel over the
  Fedora wallpaper is more than 2% off (docs/DECISIONS.md, S5). Fitted at
  output scale 1.25 only.
- **Dither.** blur-my-shell adds a dither of at most one 8-bit level to its
  blur. gniri does not.
- **Fades.** When a blurred surface fades, niri keeps blurring at full
  strength until the surface is gone; blur-my-shell fades the blur with it.

## Panel

- **Overview, around the clock.** niri's overview stacks workspaces
  vertically, so the workspace above the active one shows through the
  transparent bar. GNOME's overview has only its background there.
- **Text metrics.** Pango measures the same string in the same font up to a
  pixel differently under GTK than under Clutter (the clock is 130x18 against
  129x19 logical pixels), so the clock's highlight pill is one pixel wider.
  Glyph shapes also differ slightly: St hints text to the pixel grid at
  fractional scales and GTK does not.
- **Edges at fractional scale.** St snaps boxes to whole physical pixels; GTK
  antialiases them where they fall. At scale 1.25 the top and bottom rows of
  a highlight pill differ in brightness.
- **Workspace dots appearing and disappearing.** GNOME scales a dot in or out
  over half a second when a workspace is added or removed. gniri adds and
  removes it at once. Switching between workspaces is animated as in GNOME.
- **Keyboard focus.** GNOME's panel can be reached with Ctrl+Alt+Tab. gniri's
  takes no keyboard focus yet.
- **Clock language.** The clock is always written in English.
- **Clock after resume.** After a suspend the clock can be stale until the
  next minute starts.
- **Menus.** Until M2 and M3 the clock and status buttons open nothing, and
  clicking the keyboard-layout indicator switches to the next layout instead
  of opening a menu.

## Status indicators

- **Real data starts in M2.** The network, volume, power-profile and battery
  icons have no backends yet. In a real session the status button shows only
  the power icon GNOME shows on a machine without a battery.

## Compositor-owned

- **Hiding the panel.** niri draws fullscreen windows above the top layer,
  which hides the panel as GNOME does, but without GNOME's slide animation.
