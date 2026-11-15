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
- **Edges at fractional scale.** St snaps boxes to whole physical pixels; GTK
  antialiases them where they fall. At scale 1.25 an edge that falls between
  two pixels is a soft line in gniri and a hard one in GNOME. This holds for
  every surface, and the pixel comparison allows for it.
- **Workspace dots appearing and disappearing.** GNOME scales a dot in or out
  over half a second when a workspace is added or removed. gniri adds and
  removes it at once. Switching between workspaces is animated as in GNOME.
- **Keyboard focus.** GNOME's panel can be reached with Ctrl+Alt+Tab. gniri's
  takes no keyboard focus yet.
- **Clock language.** The clock is always written in English.
- **Clock after resume.** After a suspend the clock can be stale until the
  next minute starts.
- **Menus.** The clock opens nothing until M3, and clicking the
  keyboard-layout indicator switches to the next layout instead of opening a
  menu.

## Text

- **Optical size.** GNOME Shell sets Adwaita Sans tighter than GTK does by
  default. The theme asks for the optical size that reproduces GNOME's
  widths, fitted for the default font (Adwaita Sans 11) at scale 1.25; with
  another font or size the spacing may be slightly off.
- **Line height.** St lays a line of 11pt text out 19px tall, Pango under
  GTK 18px. Labels whose height matters get the pixel back as padding.
- **Glyph shapes** differ slightly: St hints text to the pixel grid at
  fractional scales and GTK does not.

## Quick settings

- **Opening.** GNOME's menus grow from 96% while they fade in. A blurred
  widget cannot be scaled (the compositor would blur the wrong region), so
  gniri's only fade and slide.
- **Submenus** open and close at once; GNOME's unfold over 200 ms.
- **Power button.** "Restart…" and "Power Off…" are missing until the dialog
  that asks first exists (M4). "Log Out…" asks through niri's own dialog.
  The lock button asks logind to lock the session, which does nothing until
  a screen locker listens (gniri's own arrives with M4).
- **Settings.** Entries such as "Power Settings" start GNOME Settings
  (`gnome-control-center`, configurable). Much of it expects
  gnome-settings-daemon and will not work under niri.
- **Wi-Fi.** A network that needs a password opens the settings application
  instead of asking (the dialog arrives with M6). A network name is listed
  once even when its access points differ in security. Hotspot, VPN, modem
  and Bluetooth-tether toggles are not there.
- **Bluetooth.** A device that is connecting shows text where GNOME shows a
  spinner. Pairing is not offered (M6).
- **Sound.** The slider ends at 100%; GNOME's over-amplification setting is
  not supported. The device list is built as GNOME's library builds it, from
  memory of that library rather than its source.
- **Night Light** is on while its toggle is, at one colour temperature from
  the config file. GNOME's follows a schedule and fades in. The toggle is
  off again after a restart.
- **Do Not Disturb** only shows its state until the notification server
  exists (M3).
- **Brightness** follows what gniri sets and what the kernel reports when
  the menu opens; a change made by another program while the menu is open is
  not shown until it is reopened.
- **Not ported:** the keyboard backlight item, background apps, screen
  recording and the accessibility, location, Thunderbolt and auto-rotate
  items.

## On-screen display

- The level bar jumps to a new value; GNOME's glides for 100 ms.
- Only the volume and brightness keys show it. GNOME also uses it for
  things gniri does not have yet (display switching, tablets).

## Compositor-owned

- **Hiding the panel.** niri draws fullscreen windows above the top layer,
  which hides the panel as GNOME does, but without GNOME's slide animation.
