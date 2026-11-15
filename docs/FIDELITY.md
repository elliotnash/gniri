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
- **Menus.** Clicking the keyboard-layout indicator switches to the next
  layout instead of opening a menu.

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
- **Do Not Disturb** is off again after a restart; GNOME keeps it in a
  setting.
- **Brightness** follows what gniri sets and what the kernel reports when
  the menu opens; a change made by another program while the menu is open is
  not shown until it is reopened.
- **Not ported:** the keyboard backlight item, background apps, screen
  recording and the accessibility, location, Thunderbolt and auto-rotate
  items.

## Date menu

- **Events, world clocks, weather.** gniri has no calendar, clock or
  weather backend. The three cards under the calendar are GNOME's as they
  look before anything is set up: the events card always says "No Events",
  and the other two only start GNOME Clocks and GNOME Weather. Those two are
  hidden when their application is not installed, as in GNOME, and can be
  turned off in the config file. The calendar marks no days as having
  events and has no week numbers.
- **First day of the week** comes from the config file (Sunday by default);
  GNOME takes it from the locale.
- **Opening.** As for quick settings: it fades and slides, and does not
  grow.
- **Language.** Month and weekday names are English.

## Notifications

- **Banner movement.** GNOME's banner grows from 90% and overshoots a
  little as it slides in. A blurred widget cannot be scaled, so gniri's
  slides and fades, without the overshoot.
- **Banner timing.** A banner stays four seconds, as in GNOME, but GNOME
  starts counting only once the user is active, and holds a banner longer
  while the pointer moves towards it. gniri does neither (it learns about
  idleness with M4). A banner that appears under the pointer opens at once;
  GNOME waits for the pointer to leave and come back.
- **Fullscreen.** GNOME holds back all but critical banners while a
  fullscreen window is shown. niri does not say when one is, so gniri shows
  them.
- **Which screen.** Banners appear on the output with the focused
  workspace; GNOME uses its primary monitor, which niri does not have.
- **Sound.** gniri plays no notification sounds, and does not claim to: an
  application that asks may play its own.
- **Per-application settings.** GNOME Settings can silence one application,
  keep its banners away or always show them in full. gniri treats all
  alike.
- **Which application.** GNOME can tell which application a notification is
  from by the windows of the process that sent it. gniri goes by the
  desktop entry or the name the notification gives; one that gives neither
  is listed under the name it does give, one group per process.
- **GTK's own interface.** GNOME Shell also serves `org.gtk.Notifications`.
  gniri does not; GTK applications then use the standard interface, which
  cannot say "high priority", only normal or critical.
- **Animations in the list.** Messages appear, leave and move at once, Clear
  does not sweep them out one after another, and a group opens and closes
  at once. While a group is open GNOME fades the rest of the list by
  distance from it; gniri dims the rest evenly. A long list has no fade at
  its top and bottom edges.
- **Links.** GNOME underlines web addresses in a notification's text and
  opens them on a click. gniri shows them as plain text.
- **Keyboard.** A banner cannot be given the keyboard focus (GNOME:
  Super+N).
- **Activation.** After a click on a notification gniri hands the
  application a token to raise its window with. Whether niri honours it
  depends on its focus-stealing rules.

## Media players

- **Not compared.** The reference capture was taken with no player running,
  so the media card is built from GNOME's stylesheet and source alone and
  has not been measured against GNOME.
- **Cover art** is shown only when the player gives a local file; GNOME
  also fetches covers from the network.
- **Raising the player.** As in GNOME, a click starts the player's
  application again, which normally brings its window up; whether it gets
  the focus is niri's decision.

## On-screen display

- The level bar jumps to a new value; GNOME's glides for 100 ms.
- Only the volume and brightness keys show it. GNOME also uses it for
  things gniri does not have yet (display switching, tablets).

## Compositor-owned

- **Hiding the panel.** niri draws fullscreen windows above the top layer,
  which hides the panel as GNOME does, but without GNOME's slide animation.
