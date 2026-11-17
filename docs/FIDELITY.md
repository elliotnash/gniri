# Where gniri differs from GNOME Shell

The goal is GNOME Shell 51 with blur-my-shell v78 at default settings.
GNOME Shell is dark unless told otherwise, and that is what was captured
and is compared; see "Light style" for the other. "Identical" means: theme values equal, layout within half a pixel, and
pixels within the thresholds in `tests/visual/thresholds.toml`. This file
lists everything known to fall outside that, and why.

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
- **Wi-Fi.** A network name is listed once even when its access points
  differ in security. Hotspot, VPN, modem and Bluetooth-tether toggles are
  not there. See "Network passwords" for the dialog that asks for one.
- **Bluetooth.** A device that is connecting shows text where GNOME shows a
  spinner. As in GNOME, the menu lists the devices that are set up and does
  not look for new ones: see "Bluetooth pairing".
- **Sound.** The slider ends at 100%; GNOME's over-amplification setting is
  not supported. The device list is built as GNOME's library builds it, from
  memory of that library rather than its source.
- **Night Light** is on while its toggle is, at one colour temperature from
  the config file. GNOME's follows a schedule and fades in. Whether it is
  on is kept in gniri's state file (see "Settings").
- **Do Not Disturb** is kept in the state file too; GNOME keeps it in a
  setting.
- **Brightness** follows what gniri sets and what the kernel reports when
  the menu opens; a change made by another program while the menu is open is
  not shown until it is reopened.
- **Not ported:** the keyboard backlight item, background apps, screen
  recording and the accessibility, location, Thunderbolt and auto-rotate
  items.

## Network passwords

gniri is NetworkManager's secret agent, as GNOME Shell is: when a
connection needs a password NetworkManager does not have, gniri's dialog
asks for it.

- **Not compared.** GNOME's dialog was not captured. gniri's is a port of
  its source, built from the parts of the authentication dialog, which is
  compared.
- **At once.** When NetworkManager asks on its own account (a saved
  password stopped working), GNOME shows a notification first and opens
  the dialog on a click. gniri opens the dialog.
- **One at a time.** Requests for several connections are asked one after
  the other; GNOME opens a dialog for each.
- **Nothing is kept in the keyring.** NetworkManager stores a Wi-Fi
  password itself unless a connection says "for this user only", which
  leaves it to the agent. GNOME's agent puts such a password in the
  keyring and finds it there next time. gniri keeps nothing: such a
  connection asks every time, and does not come up by itself.
- **No VPN passwords.** GNOME hands a VPN's questions to a program the
  VPN's plugin brings. gniri answers that it has none; NetworkManager then
  asks another agent if one runs (`nm-applet`, say).
- **Enterprise networks** (a user name, a certificate) that are not set up
  yet open the settings application, as in GNOME, but on its Wi-Fi page
  rather than on a dialog for that network.
- **While locked** GNOME has no agent, and a connection that needs a
  password fails. gniri's agent stays and answers that nobody answered,
  which comes to the same.

## Bluetooth pairing

GNOME Shell takes no part in pairing: GNOME Settings does it all, and
answers BlueZ's questions only while its Bluetooth panel is open. gniri is
BlueZ's agent all the time.

- **The dialog** is gniri's own. Its texts are those of gnome-bluetooth's
  pairing dialog, which GNOME Settings shows as a window; here they are in
  the shell's dialog. The count of digits typed so far on a keyboard is
  not shown.
- **Finding devices.** gniri does not search for new devices or start a
  pairing. That is done where GNOME does it, in GNOME Settings, or with
  `bluetoothctl`. Both bring an agent of their own, which BlueZ asks about
  a pairing they start. gniri's answers when a device starts the pairing
  (unless one of those has since made itself the agent to ask), and for a
  pairing started by a program that has no agent.
- **A device that connects by itself.** One that is paired is let in
  without a question; one that is not is asked about. gnome-bluetooth's
  agent lets in only an input device that is paired or trusted, and turns
  every other away.

## Date menu

- **Events, world clocks, weather.** gniri has no calendar, clock or
  weather backend. The three cards under the calendar are GNOME's as they
  look before anything is set up: the events card always says "No Events",
  and the other two only start GNOME Clocks and GNOME Weather. Those two are
  hidden when their application is not installed, as in GNOME, and can be
  turned off in the config file. The calendar marks no days as having
  events and has no week numbers.
- **First day of the week** is the locale's, as in GNOME, unless the config
  file names one.
- **Opening.** As for quick settings: it fades and slides, and does not
  grow.

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
  that output is dimmed. GNOME dims every monitor. If that output is
  unplugged, a password or pairing dialog opens again on another (what was
  typed is gone), and the end-session dialog starts its minute over; with
  no output left it is taken as cancelled.
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
- **Compared over the real wallpaper only.** GNOME's lock screen was
  captured once, by accident, so there are no flat-wallpaper pixels to
  compare with. The blurred background is the overview's (see "Blur").

## Overview

The overview is niri's. gniri adds GNOME's search entry, dash, app grid and
search results to it and takes none of niri's own away.

- **The workspaces.** They are a column, one above the other, each with
  its windows in a row that scrolls; GNOME has them side by side. A
  workspace has no rounded corners, windows carry no title or close
  button, and the neighbours of the workspace in the middle show at the
  top and the bottom of the screen: behind the bar, and beside the dash.
- **Size.** niri zooms out to at most three quarters; `gniri.kdl` asks for
  0.7, which keeps the middle workspace clear of the entry and the dash
  on screens 800 pixels high and more. GNOME's is about 0.8 and fits the
  screen exactly.
- **Above the app grid** GNOME shows the workspaces as a strip of small
  pictures. niri's cannot be drawn there. The grid is where GNOME has it
  and the strip's place stays empty.
- **Behind the app grid and a search** GNOME has shrunk or hidden the
  workspaces. niri's stay, so gniri draws the overview's background once
  more in front of them. The bar is not covered: a workspace that
  reaches behind it still shows there.
- **Behind the search entry** a neighbouring workspace can pass. The
  entry then shows that workspace's wallpaper, blurred, and never its
  windows; in GNOME nothing is ever behind it.
- **Opening and closing.** The dash slides up from the screen edge as in
  GNOME, and the entry slides out from under the bar where GNOME fades it
  in (see "Blur": a fading blurred widget is blurred at full strength
  throughout). The change to the app grid and back is at once; GNOME
  shrinks the workspaces and fades the grid in over a quarter of a
  second. None of it follows the fingers of a touchpad swipe.
- **Keys.** While the overview is open gniri has the keyboard, so that
  typing searches. Escape, Enter and the arrow keys do what niri's do
  when nothing is typed; anything else niri handles in its overview
  without a key binding does not reach it. Keys bound in niri's config
  work as always.
- **More than one output.** The entry and the dash are on every output
  that has a bar, and all show the same search. GNOME has them on the
  primary monitor only.

## Dash

- **Favourites** are the config file's list until something is dragged to,
  within or out of the dash; from then on they are the state file's,
  until the config file is given another list. An item has no menu: no
  "Pin to Dash", "New Window" or "Quit", no list of its windows.
- **Dragging** works as in GNOME: to the dash to pin, within it, onto the
  apps button to unpin ("Unpin"). Applications cannot be dragged from
  the search results, nor onto a workspace to open there. An item makes
  room and is moved at once where GNOME's grow and shrink.
- **A click** on an application that runs goes to the window of its that
  was used last. GNOME shows all its windows when there are several, and
  starts a new one on a middle click or with Ctrl held.
- **Which application a window belongs to** goes by the window's
  application id and the desktop entries' names and `StartupWMClass`.
  GNOME also goes by the process and by what the application's own
  sandbox says, so an oddly named window finds its icon there and can
  show as an item of its own here, under its window's title and a plain
  icon.
- **The order of the running applications** that are no favourites is the
  order their first windows opened in.
- **Items come and go at once**; GNOME's grow in and shrink away. A
  starting application shows no spinner on its icon, and its icon does
  not zoom out as it starts.
- **The name over an item** is built from GNOME's stylesheet and source;
  the capture has none to compare with.

## App grid

- **Order and folders** are GNOME's to start with: its default order and
  its two folders, "Utilities" and "System". Dragging rearranges them as
  in GNOME (a place in the grid, onto another application to make a
  folder, into a folder, out of one, a page turned at the grid's edge),
  and the arrangement is kept in gniri's state file, not in GNOME's
  settings: an arrangement made in GNOME does not carry over.
- **Pages are always full.** GNOME keeps an icon on the page it was put
  on, and a page can end before it is full; here the icons flow from one
  page to the next.
- **A folder's dialog** fades in and out; GNOME's also grows out of the
  folder's icon and back. The blur behind it is gniri's own: the
  compositor cannot blur what is on gniri's surface under another part of
  it, so the dialog draws the overview again behind itself, blurred and
  dimmed as blur-my-shell's blur would have it.
- **A new folder** is called by the category its applications share if
  a desktop directory file names it, else "Unnamed Folder", as in GNOME;
  its id is a number where GNOME's is random.
- **A long name** is cut short. GNOME shows all of it, on two lines,
  while the pointer is on the icon.
- **Turning the page** by its arrows, by scrolling, by a click on a page's
  dot and by Page Up and Page Down. There is no touchpad or touch-screen
  swipe, and no strip at the screen's side that shows the next page while
  something is dragged.
- **Applications that want a terminal** run in `xdg-terminal-exec` if it
  is installed, else in the first of a short list of terminals that is.
- **What is listed.** Entries with `OnlyShowIn=GNOME` are listed, since
  gniri stands in for GNOME Shell: Settings is one. Some of those expect
  more of GNOME than gniri provides.

## Search

- **Applications** are found as GLib finds them: by the words of their
  name, their program's name, their keywords, generic name and comment,
  best match first. GNOME then puts the applications used most first;
  gniri keeps no count and orders equal matches by name.
- **Not found:** GNOME's own results for "power off", "lock" and the like,
  and its offer to search GNOME Software or the web when nothing matches.
- **Providers.** The applications asked are those that install a search
  provider for GNOME Shell, in GNOME's default order; which are asked is
  in gniri's config file, not in GNOME Settings. A provider that GNOME
  only asks once the user has turned it on there is never asked. One that
  has not answered after ten seconds is given up on for that search.
- **Every letter starts a search.** GNOME waits 150 ms after the first
  for the typing to pause, and narrows a provider's previous results
  where it can; gniri asks afresh each time, and drops the answers to
  what was typed before.
- **Pictures.** A result's picture is shown if the provider names an icon
  or a file or sends pixels; one sent as an encoded image is left out.
- **Keys.** Down from the applications goes to the first result under
  them, wherever the selection was in the row; Tab does nothing.
- **Clipboard.** GNOME copies some results (a character, a calculation)
  when they are chosen. gniri leaves that to the application the result
  opens in.
- **While searching** the status text has no spinner beside it.

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

## Light style

GNOME Shell has a light stylesheet, and shows it in one case only: the
desktop's colour scheme is "prefer light", which no switch in GNOME sets
(the Dark Style switch changes between "prefer dark" and no preference,
and the shell is dark for both). gniri follows the same rule, and has a
setting for the other three GNOME knows (`[appearance] style`): always
dark, always light, or light unless the switch says dark, which is GNOME
Classic's rule and the one that makes the shell follow the switch.

- **Not compared.** GNOME was never captured in its light style. gniri's
  is the same theme compiled for the other variant, as GNOME's own is,
  and blur-my-shell's light styles ported from its source; the theme's
  base colours are checked against GNOME's compiled light stylesheet.
- **The blur is the dark style's.** blur-my-shell keeps its blur, dimmed
  to 60%, and lays light tints on it; it has a brighter blur for the
  light style which is off by default, and gniri does not have it. So the
  overview's background is as dark as ever, and the extension's black
  names under the app grid's icons are hard to read on it, there as here.
- **The lock screen, the on-screen display's bar and the overview's own
  texts** are dark in both, as in GNOME.
- **A changed `style` setting** reaches the shell at once, and the lock
  screen and the polkit agent when they next start; all three follow the
  desktop's colour scheme as it changes.

## Language

gniri has no translations of its own. Its texts are GNOME's, word for
word, and it looks them up in the catalogs GNOME installs: GNOME Shell's,
gnome-desktop's (the clock), gnome-bluetooth's (the pairing dialog) and
NetworkManager's (the names of the kinds of network). Where those are
installed for the user's language, gniri is in that language.

- **gniri's own texts stay English**: the few GNOME has no text for, such
  as "Disconnecting…" beside a Bluetooth device (GNOME shows a spinner)
  and what the lock screen says when PAM cannot be asked.
- **Which language.** Texts and the formats dates are written in follow
  `LC_MESSAGES` (and `LANGUAGE`); the names of months and weekdays follow
  `LC_TIME`. GNOME translates a date's format in the language of
  `LC_TIME`, so with two different languages set there a date can come
  out in the one's order with the other's names.
- **Applications' names** are in the user's language where their desktop
  entries have them, and a search finds an application by its English
  name too. Entries that leave their translation to a catalog of their
  own (`X-GNOME-Gettext-Domain` and the like) show in English.
- **Right-to-left languages** have their texts; the layout is not
  mirrored.
- **Not compared.** The reference was captured in English.

## Settings

- **What the shell itself changes** (how the applications are arranged,
  Do Not Disturb and Night Light) is kept in a state file,
  `~/.local/state/gniri/state.toml`, which is the shell's to write. One
  that cannot be read is set aside and the shell starts afresh.
- **One file.** What GNOME keeps in GSettings and shows in GNOME Settings
  is, as far as gniri has it, in `~/.config/gniri/config.toml`
  (docs/CONFIG.md). A change to the file takes effect at once. GNOME
  Settings' own pages for the same things (the dash's favourites, the
  search providers, blank-screen delay, night light) change nothing in
  gniri.
- **The one exception** is the dark style, which is the setting
  applications read.

## Compositor-owned

- **Hiding the panel.** niri draws fullscreen windows above the top layer,
  which hides the panel as GNOME does, but without GNOME's slide animation.
- **Power button and lid.** niri and logind handle them. GNOME's own
  handling (asking, suspending, ignoring) is not reproduced.
