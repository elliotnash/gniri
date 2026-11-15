# gniri

A desktop shell for the [niri](https://github.com/YaLTeR/niri) compositor that
looks like GNOME Shell 51 with the blur-my-shell extension, built with GTK 4
and libadwaita in Rust.

What it is:

- the wallpaper, the blurred overview backdrop and the top bar (workspace
  dots, clock, status icons, keyboard layout, screen-sharing indicator);
- the quick settings menu with its submenus, the volume and brightness
  display, and the services behind them: sound, Wi-Fi and wired networks,
  Bluetooth, battery and power mode, brightness, airplane mode, dark style
  and night light;
- notifications (gniri is the notification server: banners, do not
  disturb), and the date menu with the calendar, the notification list and
  media player controls;
- what a session needs to be lived in: the lock screen (password and
  fingerprint), dimming, blanking and locking when idle, locking before
  sleep, the dialog that asks before restarting, powering off or logging
  out, and the polkit authentication agent;
- GNOME's dress for niri's overview: the search entry, the dash with the
  favourite and the running applications, the app grid, and a search that
  finds applications and asks the ones that offer results of their own
  (Files, Calculator, Characters, Software and so on);
- the dialogs the system asks through: for a Wi-Fi network's password, and
  for the questions of a Bluetooth pairing.

It talks to the system directly: PipeWire or PulseAudio for sound,
NetworkManager, BlueZ, UPower, a power-profiles daemon and logind. Whatever
is missing is left out of the menu. It needs no gnome-settings-daemon and
no GNOME Shell.

Where it differs from GNOME Shell, and why, is written down:
`docs/FIDELITY.md`.

**Status.** Written for and used on one machine: Fedora 45, niri 26.04, a
laptop at scale 1.25. Dark style only, and its texts are English. Expect
rough edges elsewhere.

## Install

It needs GTK 4.24, libadwaita 1.10 and niri 26.04 (it is built against
that release's IPC, which a later niri may change), and to build, Rust
1.93 or later. On Fedora:

```
sudo dnf install rust cargo gtk4-devel libadwaita-devel gtk4-layer-shell-devel \
    glib2-devel pulseaudio-libs-devel pam-devel
cargo xtask install
```

That builds gniri and installs it under `~/.local`, which needs no
privileges:

```
~/.local/bin/gniri
~/.local/share/gniri/gniri.kdl                 the fragment niri's config includes
~/.local/share/systemd/user/gniri.service
~/.local/share/doc/gniri/
```

For the whole system, stage it and copy the result as root (so that cargo
never runs as root); this also installs the lock screen's PAM services:

```
cargo xtask install --prefix /usr --destdir target/stage
sudo cp -a target/stage/. /
```

`cargo xtask uninstall`, with the same options, takes it all away again.

## Set up

Add one line to `~/.config/niri/config.kdl`, after your own `binds`, with
the path `cargo xtask install` printed:

```kdl
include "/home/you/.local/share/gniri/gniri.kdl"
```

The included file sets niri's blur to match blur-my-shell (this changes blur
for every app) and the overview's zoom to leave room for the dash, places
gniri's surfaces, and binds the volume and brightness keys to `gniri msg
volume up|down|mute` and `gniri msg brightness up|down`, Super+A to `gniri
msg apps`, Super+Alt+L to `gniri msg lock` and Ctrl+Alt+Delete to `gniri msg
power-off`. Those binds only win over binds of the same keys that come
before the `include` line.

Then have niri start gniri. As a systemd user service, which starts it
again if it fails:

```
systemctl --user daemon-reload
systemctl --user add-wants niri.service gniri.service
```

or with a line in niri's config: `spawn-at-startup
"/home/you/.local/bin/gniri"`.

gniri takes over several jobs a session has one program for. Stop whatever
did them before, or the two get in each other's way:

- **another bar** (waybar and the like);
- **the notification daemon** (mako, dunst, swaync). Whichever starts
  first has the name `org.freedesktop.Notifications`; if that is not gniri
  it says so on stderr and shows none;
- **the idle and lock programs** (swayidle, swaylock);
- **the polkit agent**, of which polkit accepts one per session;
- **`nm-applet`**, if you want gniri's dialog to be the one that asks for a
  Wi-Fi password.

## Settings

Settings go in `~/.config/gniri/config.toml`; `docs/CONFIG.md` describes
every one, and `data/config.example.toml` is a file to start from. Without
a `[wallpaper]` setting gniri draws no wallpaper.

The file is read again whenever it changes: save it and the change is
there. A mistake in it changes nothing and is told of in a notification.

## Using it

### The overview

gniri adds to niri's overview; it does not replace it. Open it as you do
now (the hot corner, the four-finger swipe, your `toggle-overview` bind, a
click on the left end of the bar): the windows are niri's, to click, drag
and scroll as before, and around them are a search entry and a dash. The
dash's last button, or Super+A, shows every installed application.

While the overview is open the keyboard is gniri's: type to search, Enter
opens the first result, the arrow keys choose another, Escape goes back a
step and then closes the overview. With nothing typed, Enter and the
arrow keys do what niri's own do there. Keys you have bound in niri's
config work as always.

Which applications are always in the dash is in the config file
(`[dash]`); the default is GNOME's. A search asks the applications that
offer GNOME Shell results over D-Bus, starting them as GNOME does;
`[search]` turns that off or leaves some out.

### Locking, idleness and passwords

gniri locks the screen itself (`gniri msg lock`, the lock button, `loginctl
lock-session`), before the machine sleeps, and after five minutes without
input, having dimmed the screen first and turning it off as it locks. The
delays are in the config file.

**Read "The real session" in `docs/TESTING.md` before you first lock**: it
says how to get back in from another terminal if the lock screen ever
fails.

Passwords are checked by PAM. The lock screen has service files of its
own, which a system-wide installation puts in `/etc/pam.d`; from the
repository:

```
sudo cp data/pam/gniri data/pam/gniri-fingerprint /etc/pam.d/
```

They are written for Fedora; the files say what to change elsewhere.
Without them gniri uses the services Fedora has for the same purpose
(`password-auth`, `fingerprint-auth`), or failing that `login`, which on
some systems waits for a fingerprint before it looks at a password.

gniri is also the session's polkit authentication agent (the dialog that
asks for a password when a program needs privileges).

### Wi-Fi and Bluetooth

A click on a network in the Wi-Fi menu joins it. If it wants a password,
NetworkManager asks gniri and gniri asks you; NetworkManager keeps the
password. A network that wants more than a password (a user name, a
certificate) has to be set up in a settings application first.

The Bluetooth menu lists the devices that are set up, as GNOME's does, and
connects and disconnects them. A new device is paired where GNOME pairs
it, in GNOME Settings, or with `bluetoothctl`. gniri answers BlueZ's
questions when a device starts a pairing itself, with a dialog that shows
the number to compare or takes the PIN.

### More than one screen

Every output gets a bar, a wallpaper and its share of the overview
(`[panel] outputs` in the config file names the ones that get a bar, if
not all should). Menus open on the output whose bar was clicked; banners
and dialogs appear on the output with the focus. Outputs can come and go
while gniri runs.

## Documentation

- `docs/CONFIG.md`: every setting.
- `docs/FIDELITY.md`: where gniri differs from GNOME Shell, and why.
- `docs/TESTING.md`: the test harness, the GNOME reference, and what to
  try by hand in a real session.
- `docs/DECISIONS.md`: design decisions and the experiments behind them.

## Licence

GPL-3.0-or-later; see `COPYING`. The theme is ported from gnome-shell
(GPL-2.0-or-later) and follows blur-my-shell (GPL-3.0); the files copied
unchanged from gnome-shell are in `crates/theme/scss/upstream` and
`crates/shell/resources/icons`.
