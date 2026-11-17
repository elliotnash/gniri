# Testing

```
cargo test --workspace          # logic, theme values, backends; no display needed
cargo xtask diff quick-settings # looks: gniri against the GNOME reference
cargo xtask check               # behaviour: gniri driven in a live session
cargo xtask check overview      # one part of it (see "What the checks cover")
cargo xtask report              # diff every component, write an HTML page
```

`diff`, `check` and `report` run gniri inside a nested niri window, so they
need a running Wayland session with the screen unlocked (a locked screen
stalls nested niri completely). You can keep working in other windows. They
also need ImageMagick (`magick`); `check` uses `wlrctl` for pointer input and
`wtype` for keys, and skips those checks without them.

Every run uses `tests/visual/scenes/config.toml`, never your own config, a
private D-Bus session bus, and a fixture in place of the system: no click in
a test turns your Wi-Fi off. It also runs in American English whatever your
own language is, since that is what the reference was captured in.

A run needs the screen unlocked. Behind GNOME's lock screen a nested
session stalls, and the harness refuses to start; behind gniri's own it
goes on slowly, the harness says so and waits five times as long for
everything, and what depends on the nested output's size (the
comparisons with the reference) is not to be trusted.

The session around it can be GNOME's or niri's own (with gniri as its
shell or without). Under niri the harness takes the nested session's
window out of the columns and sizes it itself, since niri would make it
as wide as a column and as high as the screen.

## Backends

`cargo test` also runs the backends against stand-ins for the system, and
skips each of these with a note when its tools are not installed:

- **UPower, NetworkManager, BlueZ**: python-dbusmock (`python3-dbusmock`) on
  a private bus (`crates/services/tests/*_bus.rs`).
- **The two agents** (for Wi-Fi passwords and for Bluetooth pairing): the
  test itself owns NetworkManager's or BlueZ's name on a private bus and
  asks the agent what the service would
  (`crates/services/tests/network_agent_bus.rs`, `bluetooth_agent_bus.rs`).
  Needs only `dbus-daemon`.
- **Sound**: a private PipeWire with null devices
  (`crates/services/tests/audio_server.rs`); your own sound server is not
  touched.
- **Notifications**: gniri's own server on a private bus, talked to with
  `notify-send` and with plain bus calls
  (`crates/services/tests/notifications_bus.rs`). Needs only `dbus-daemon`.
- **Media players**: a bare python-dbusmock object standing in for a player
  (`crates/services/tests/mpris_bus.rs`).
- **The session**: python-dbusmock's logind and a stand-in for the screen
  locker (`crates/services/tests/session_bus.rs`): locking on request,
  sleep waiting for the lock, a locker that dies.
- **The polkit agent**: a bare python-dbusmock object standing in for
  polkit and a shell script for its helper
  (`crates/services/tests/polkit_bus.rs`).

- **Calendar events**: the test itself owns GNOME Shell's calendar
  server's name on a private bus and answers as it does
  (`crates/services/tests/events_bus.rs`). Needs only `dbus-daemon`.

- **Search providers**: a bare python-dbusmock object standing in for an
  application that answers searches, with its key file and desktop entry
  in a directory of the test's own
  (`crates/services/tests/search_bus.rs`).

One test asks the system's real PAM, with a wrong password, and so only
runs on request: `cargo test -p gniri-services -- --ignored real_pam`.

To see what a backend reports from the real system, without changing
anything: `cargo run -p gniri-services --example watch -- power` (or
`network`, `bluetooth`, `brightness`, `audio`, `rfkill`, `mpris`,
`session`, `idle`, `apps`, `events`). `events` asks the calendar server
for the next thirty days, which starts it and Evolution Data Server. `-- search calc` asks the installed search
providers for "calc", which starts them, as GNOME Shell's search does.

## What is compared

| Tier | What | Where |
|---|---|---|
| 0 | Theme values against GNOME's computed styles | `crates/theme/tests/` |
| 1 | Each widget's box against its GNOME actor's | `xtask diff` |
| 2 | Pixels over flat wallpapers, text excluded | `xtask diff` |
| 3 | Low-passed pixels over the real wallpaper | `xtask diff` |

The pass marks are in `tests/visual/thresholds.toml`.

The reference is in `tests/visual/reference/gnome-51.1/`: `metrics/` holds
actor trees with their boxes and computed styles, `shots/` the matching
screenshot crops, both cut from a capture of the running GNOME Shell by
`tools/gnome-ref/`. The capture needs GNOME's unsafe mode and is only redone
when GNOME is upgraded.

The nested output is 1280x800 at scale 1.25; the reference screen is
1792x1120 at the same scale. Nothing is resized. Each component has a root
(the panel, a menu, the display): boxes and pixels are compared relative to
it, where it sits is compared as its distance from the screen edges it is
attached to, and for the real wallpaper the nested output is given the part
of the wallpaper that lies under it on the reference screen.

GNOME snaps every box to whole physical pixels; GTK antialiases an edge
where it falls. So in tier 2 a pixel only counts as different if it is
outside the range of colours the other picture has within one pixel of it.
A wrong colour, a missing line or an edge two pixels out still counts.

Screenshot runs start gniri with `--no-pointer`: your pointer passing over
the nested window must not show up as a hovered button.

`cargo xtask check` ends with a session that has no fixture, in which gniri
serves notifications on the nested session's own bus and `notify-send` is
run as any application would run it. That session reads the state of the
real system (battery, network) but changes nothing.

### What depends on the size of the screen

GNOME decides some things by the height of the screen: how large the
dash's icons are, how many icons a page of the app grid holds and at what
size, how far apart the overview's parts are. The nested output is lower
than the reference screen, so there GNOME's own rules give another
answer than the capture shows: 48px icons in the dash, 64px in the grid.

Those rules are plain arithmetic (`crates/shell/src/overview/layout.rs`)
and are tested as such, against the boxes of GNOME's own actors: every
icon of the captured app grid is where the arithmetic puts it
(`metrics/app-grid.json`, which is only measured and has no pictures).
What does not depend on the screen is compared as everything else is:

- the dash, with its icon size pinned in the test configuration to the
  64px the capture has;
- an application's tile, among the search results, where GNOME never
  makes it smaller;
- the page dots and the arrow of the app grid, and the lists of search
  results. A map says `placement = false` for these: their distance from
  the screen edges is not compared, and for the real-wallpaper tier the
  nested output is given the wallpaper that lies behind GNOME's.

The applications in these scenes are the scenes' own, and so is what a
search finds: under a fixture no desktop entry is read and nothing is
asked over D-Bus. The reference has the user's applications blacked out,
icons and names: an extract says `apps`, and the comparison skips those
boxes while still comparing where they are. A scene opens a window of an
application with `window = { color = "…", app-id = "…" }`.

The test session uses a smaller overview zoom than `data/niri/gniri.kdl`
sets (`tests/visual/niri/test.kdl` says why).

Any scene can be looked at in the light style, which GNOME was never
captured in: `cargo xtask shot look --scene quick-settings --light
--wallpaper '#808080'`.

Some scenes have no GNOME capture to compare with and exist to be looked
at: `cargo xtask shot look --scene date-menu-media --wallpaper '#808080'`
(a media player's card), `date-menu-group` (an opened group),
`date-menu-empty`, `quick-settings-busy`, `end-session` (the dialog that
asks before powering off), `wifi-password` (the one that asks for a
network's password), `bluetooth-pairing` (a number to compare),
`app-grid-folders` and `app-folder` (a folder's dialog open), and
`date-menu-events` (a calendar with events, and world clocks).

A scene that `extends` another adds to its fixture service by service:
`[fixture.apps.layout]` keeps the base scene's installed applications. A
scene's applications are in name order unless it gives an arrangement;
the real shell starts from GNOME's.

### The lock screen and the dialogs

The lock screen and the polkit agent are processes of their own. A scene
starts one with a `spawn` step, with the scene's fixture; the process then
answers `dump-tree` and `sync` on a socket of its own, and the harness
reads its widgets along with the shell's. A scene's lock screen takes the
scene's password and never asks PAM; a scene's agent shows the scene's
request and never talks to polkit. Only a debug build accepts a fixture
for the lock screen.

niri takes no screenshot of a locked session. So the scenes that are
compared with GNOME (`lock`, `lock-prompt`) show a preview: the same
widgets on an ordinary surface, nothing locked. `cargo xtask check lock`
locks the nested session for real and goes by the widget dump and a
virtual keyboard: a wrong password, the right one, the desktop back.

GNOME's own lock screen was only captured over the real wallpaper, so
`lock-clock` and `lock-prompt` have no flat-wallpaper tier. The user's
name and picture are blacked out of the committed references.

### What the checks cover

`cargo xtask check` runs these parts, each in a session of its own; a
part's name after `check` runs that one alone.

| Part | What it drives |
|---|---|
| `panel` | Workspace dots, the overview's look, fullscreen, with real services |
| `overview` | The dash, the app grid, the search and its keys |
| `quick-settings` | The menu, its toggles and sliders, the on-screen display |
| `notifications` | Banners, the list, do not disturb, sounds |
| `calendar` | The date menu's events and world clocks |
| `notifications-bus` | The same over D-Bus, with `notify-send` |
| `session` | The end-session dialog and what idleness does |
| `lock` | The lock screen, locking the nested session for real |
| `polkit` | The authentication dialog |
| `agents` | The Wi-Fi password and Bluetooth pairing dialogs |
| `config` | The config file changing under a running shell |
| `outputs` | Outputs going and coming back, and one changing its scale |
| `folders` | App folders, and dragging icons in the grid, a folder and the dash |
| `language` | gniri in German: GNOME's translations, dates, sentences with numbers |

`language` asks gettext what each text should read and compares, so it
checks that GNOME's translations are found and not what they say; it is
skipped where GNOME Shell's German catalog or the German locale is not
installed. A unit test beside it (`cargo test -p xtask`) reads every text
gniri asks a translation for out of the source and looks for it in GNOME's
installed catalogs: a text that is a character off would never be
translated, and nobody reading English would see why.

`folders` drags with a pointer of the harness's own
(`crates/xtask/src/pointer.rs`, a client of the nested compositor's
virtual pointer), since `wlrctl` can click but not hold a button down. It
leaves two pictures of drags under way, `target/visual/drag-room.png` and
`drag-folder-preview.png`, to look at.

Two of the others need saying more about.

**`config`** writes a config file of its own and rewrites it step by
step, moving each version into place as an editor does. With a fixture
for services, what a backend was told is read from the command log. Its
last steps run without a fixture, on the nested session's own bus, where
gniri is the notification server: a file with a mistake in it has to
produce a banner.

**`outputs`**: the nested compositor has one output, and it cannot be
unplugged (`niri msg output winit off` does nothing there). So `gniri msg
debug unplug` has the shell behave as if every output had gone, and
`--back` as if they had returned. From the moment GTK reports the change
that is the same code a real unplugging runs: every surface of the output
is torn down, and built again. What it cannot show is what the
compositor and GTK do before that moment, and anything that needs two
outputs at once; those are on the list for the real session below. The
change of scale in the same part is the compositor's own.

## Adding a component

1. Add it to `EXTRACTS` in `tools/gnome-ref/extract.py`, run the script,
   look at the files it wrote and commit them. Text that is private (a
   Wi-Fi name) is listed under `redact`: it is blacked out of the pictures
   and the comparison skips it.
2. Write a scene, `tests/visual/scenes/<name>.toml`: the fixture (frozen
   clock, service state) and the steps that bring the session to the state
   the reference shows: windows, niri actions and `gniri msg` requests. The
   format is described in `crates/xtask/src/scene.rs`.
3. Write the map, `tests/visual/map/<name>.toml`: our surface and root
   widget, which widget is which actor, and which regions to compare. See
   `quick-settings.toml`; selectors are described in
   `crates/xtask/src/tree.rs`.
   A surface that the compositor places below the panel (a banner, the
   overview) says so with `below = "gniri-panel"`: a widget dump cannot
   tell where its surface was put. A component whose parts are not all
   inside its root names a widget above it as `tree`, where the pairs'
   selectors then start (the dash's items are beside its background).
4. `cargo xtask diff <name>`, then open the pictures under
   `target/visual/report/<name>/`: ours on top, GNOME in the middle,
   differing pixels marked at the bottom.

## Looking inside a running shell

```
cargo xtask nest --scene quick-settings --wallpaper '#808080'   # until Ctrl-C
gniri msg debug dump-tree        # every mapped widget
gniri msg debug commands         # what the shell asked its services for
gniri msg debug open-submenu wifi
gniri msg debug open-menu date-menu
gniri msg debug notify --app Chat --action reply=Reply "A message" "Its text"
gniri msg debug open-group debug:Chat   # a group of the date menu's list
gniri msg power-off              # the end-session dialog
gniri msg debug close-dialog
gniri msg debug idle dim         # as if nobody had touched anything for that long
gniri msg debug idle dim --resumed
gniri msg apps                   # the overview, on the app grid
gniri msg debug show-apps --off  # back to the windows
gniri msg debug search calc      # as if typed into the overview
gniri msg debug page 1           # the app grid's second page
gniri msg debug pairing confirm --device Phone --pin 482193   # as if BlueZ asked
gniri msg debug unplug           # as if every output had been unplugged
gniri msg debug unplug --back
```

`debug notify` delivers a notification as if an application had sent it.
Under a fixture that is the only way to get one: no notification server
runs there.

`nest` prints the sockets of the nested session. With `GNIRI_SOCKET` set to
the `gniri_socket` path in `target/visual/logs/nest.ready`, `gniri msg`
reaches that shell. Without `--scene` the nested shell uses the real
services: what you click there happens. It is then also an agent of the
real NetworkManager and BlueZ for as long as it runs: joining a Wi-Fi
network from its menu asks for the password in its window, and a
Bluetooth pairing question may come up there.

## The real session

What no nested session can show, and so what is checked by hand after
logging in to niri with gniri as the shell: locking with the real password
and a fingerprint, suspend and lid close, the idle chain on the real
backlight, `pkexec`, restart, power off and log out, a real Wi-Fi password
and a real Bluetooth pairing, and a second monitor.

Before the first try, know the way back in. The lock is the compositor's:
if the lock screen ever fails to take a password, or dies, the session
stays locked.

1. Switch to another terminal (Ctrl+Alt+F3) and log in.
2. `loginctl unlock-session <id>` (`loginctl list-sessions` shows the id)
   tells a running lock screen to unlock.
3. If no lock screen runs (the screen is blank), start one for the locked
   session and unlock it there, or on its prompt:
   `WAYLAND_DISPLAY=wayland-1 gniri lock`.
4. As a last resort `loginctl terminate-session <id>` ends the session.

Then, in the session:

- `gniri msg lock` (or Super+Alt+L): the clock, a key, the password. A
  wrong one is refused after PAM's delay. With a finger enrolled, the
  reader unlocks while the prompt shows.
- `loginctl lock-session` from a terminal locks the same way.
- Close the lid or `systemctl suspend`: on waking, the lock screen is
  there before the desktop ever shows.
- Leave the machine: the screen dims after four minutes, turns off and
  locks after five (`[idle]` and `[lock]` in the config file shorten this
  for a test), and a locked screen turns off after 15 seconds.
- `pkexec true` in a terminal: the authentication dialog, a wrong
  password, the right one, Escape.
- The power menu: "Restart…" and "Power Off…" ask first and count down;
  Cancel and Escape stop them. "Log Out…" ends the session.
- The overview, by the hot corner, the touchpad's four-finger swipe and
  your own bind: the entry and the dash come with it and go with it; a
  window can still be clicked and dragged between them.
- Type a few letters there: your applications, and under them what Files,
  Settings, Calculator and the others find. Enter starts the first; the
  new window has the focus.
- A click on a dash icon whose application runs goes to its window, on
  whatever workspace; on one that does not, starts it.
- Super+A: every application, in pages; scrolling and the arrows turn
  them. On a second monitor the same shows, and the keyboard is with the
  output that has the focus.
- A Wi-Fi network you have never joined, from the Wi-Fi menu: the dialog
  asks for its password, a wrong one makes NetworkManager ask again, the
  right one connects, and the network is known afterwards. Cancel leaves
  it unconnected. (`nmcli connection delete <name>` forgets it for another
  try.)
- Bluetooth: put a phone in pairing mode and start the pairing from the
  phone, with GNOME Settings closed. gniri's dialog shows the number the
  phone shows; Confirm pairs, Cancel does not. For a device that only
  takes pairings, `bluetoothctl` with `agent off` and then `pair <address>`
  leaves the questions to gniri.
- Change `~/.config/gniri/config.toml` while gniri runs: the clock's
  format, the wallpaper, the dash's favourites. Each shows at once. Put a
  mistake in: a banner says where, and nothing changes.
- With a second monitor: plug it in and out while a menu, the overview
  and a dialog are open in turn, on either screen. Each output has its
  bar and wallpaper again when it returns; nothing is left behind on the
  one that stayed, and gniri keeps running. Close the lid with the
  external monitor attached, and open it again.
- `journalctl --user -b | grep gniri` afterwards: nothing unexpected.
