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
- **Power button.** "Log Out…" ends the session by asking niri to quit.
  "Switch User…" is not there. See "Dialogs" for the dialog that asks first.
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
  while the pointer moves towards it. gniri does neither. A banner that
  appears under the pointer opens at once;
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

## Dialogs

- **Border.** A dialog's one-pixel border is the one edge where the
  difference between St's pixel snapping and GTK's antialiasing shows in
  the pixel comparison: it is brighter against its surroundings than any
  other edge (tests/visual/map/polkit-dialog.toml).
- **One output.** The dialog opens on the output with the focus, and only
  that output is dimmed. GNOME dims every monitor.
- **Opening.** The blur behind the dialog and its shade does not fade in
  with them (see "Blur").
- **End-session dialog: not compared.** GNOME's was not captured. It is
  built from the same parts as the authentication dialog, which is
  compared, and from GNOME's source.
- **End-session dialog: contents.** GNOME lists the applications that
  would lose work and the other users who are logged in, offers to install
  pending updates, warns about a low battery before updating and offers
  the boot menu while Alt is held. gniri's dialog has the question, the
  countdown and the two buttons.
- **Who acts.** In GNOME gnome-session ends the session, giving
  applications time to object. gniri asks logind to restart or power off,
  and ends the session by having niri quit; applications are not asked.

## Authentication (polkit)

- **One user.** As in GNOME, the dialog asks for one user's password: the
  user's own if polkit accepts it, else root's.
- **While locked.** GNOME holds a request back until the screen is
  unlocked. gniri's dialog opens at once and is under the lock screen
  until then, which comes to the same.
- **The helper.** Passwords are checked by polkit's own helper. gniri
  starts the setuid helper, or uses the socket systemd offers for it where
  the distribution enables that; the second way has not been tried on a
  system that has it.

## Lock screen

- **No panel.** GNOME's lock screen keeps the right end of the panel
  (network, sound, battery, with a reduced quick settings menu). gniri's
  shows the clock and the prompt alone: the volume and brightness keys
  still work, but nothing shows the battery.
- **No notifications.** GNOME lists who sent notifications while locked,
  at the bottom of the screen. gniri's lock screen does not, and no banner
  shows over it. They are in the date menu after unlocking.
- **Changing between clock and prompt.** They cross-fade. GNOME also
  shrinks one away upwards as the other grows in from below, follows a
  swipe on a touch screen, and reacts to scrolling.
- **Locking and unlocking** happen at once. GNOME fades the screen out
  before an idle lock and slides the shield away after unlocking.
- **Every output asks.** Each output has the clock and, after a key press
  or click there, its own prompt. GNOME has the prompt on one monitor.
- **Fingerprint.** A finger is waited for while a prompt shows, through
  PAM, and what the fingerprint module says is shown under the entry as it
  comes. GNOME has its own wording and a menu of the ways to unlock.
  Smart cards, web login and the other GDM mechanisms are not supported.
- **Messages.** A wrong password gets GNOME's sentence; anything else is
  PAM's own text. The hint under the clock always says "Click or press a
  key".
- **Not there:** "Switch User…", the on-screen keyboard, the
  screen-time-limit shield, media controls.
- **Language.** The clock and the texts are English.
- **Compared over the real wallpaper only.** GNOME's lock screen was
  captured once, by accident, so there are no flat-wallpaper pixels to
  compare with. The blurred background is the overview's (see "Blur").

## Idle and power

- **Dimming** sets the backlight to 30% at once, as
  gnome-settings-daemon's default does over a short fade, and only where
  there is a backlight gniri can set.
- **No automatic suspend.** GNOME suspends an idle laptop after 15
  minutes on battery. gniri turns the screens off and locks; logind
  suspends on a closed lid.
- **What holds idleness off.** niri counts the idle time and honours the
  Wayland idle-inhibit protocol (video players, Firefox). Programs that
  only ask over D-Bus (`org.freedesktop.ScreenSaver`, GNOME's session
  manager) are not heard.
- **Settings** are in gniri's config file (`[idle]`, `[lock]`), not in
  GNOME Settings.

## Compositor-owned

- **Hiding the panel.** niri draws fullscreen windows above the top layer,
  which hides the panel as GNOME does, but without GNOME's slide animation.
- **Power button and lid.** niri and logind handle them. GNOME's own
  handling (asking, suspending, ignoring) is not reproduced.
