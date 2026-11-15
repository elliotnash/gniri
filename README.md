# gniri

A desktop shell for the [niri](https://github.com/YaLTeR/niri) compositor that
looks like GNOME Shell 51 with the blur-my-shell extension, built with GTK 4
and libadwaita in Rust.

**Status: early.** Five milestones are done:

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
  (Files, Calculator, Characters, Software and so on).

Still to come: the Wi-Fi password and Bluetooth pairing dialogs.

## Build

Fedora:

```
sudo dnf install rust cargo gtk4-devel libadwaita-devel gtk4-layer-shell-devel \
    glib2-devel pulseaudio-libs-devel pam-devel
cargo build --release
```

Developed against niri 26.04, GTK 4.24 and libadwaita 1.10.

## Try it

Add to `~/.config/niri/config.kdl`, with your own path:

```kdl
include "/path/to/gniri/data/niri/gniri.kdl"
spawn-at-startup "/path/to/gniri/target/release/gniri"
```

The included file sets niri's blur to match blur-my-shell (this changes blur
for every app) and the overview's zoom to leave room for the dash, places
gniri's surfaces, and binds the volume and brightness keys to `gniri msg
volume up|down|mute` and `gniri msg brightness up|down`, Super+A to `gniri
msg apps`, Super+Alt+L to `gniri msg lock` and Ctrl+Alt+Delete to `gniri msg
power-off`. Those binds expect `gniri` on your `PATH`, and only win over
binds of the same keys that come before the `include` line. Remove any
other bar or launcher you start.

Instead of the `spawn-at-startup` line gniri can run as a systemd user
service, which starts it again if it fails: see `data/systemd/gniri.service`.

gniri talks to the system directly: PipeWire or PulseAudio for sound,
NetworkManager, BlueZ, UPower, a power-profiles daemon and logind. Whatever
is missing is left out of the menu. It needs no gnome-settings-daemon.

gniri serves `org.freedesktop.Notifications` itself. Stop any other
notification daemon (mako, dunst, swaync): whichever starts first has the
name, and if that is not gniri it says so on stderr and shows none.

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
input, having dimmed the screen first and turning it off as it locks. Stop
swayidle, swaylock and the like: two programs doing this get in each
other's way. The delays are in the config file.

**Read "The real session" in `docs/TESTING.md` before you first lock**: it
says how to get back in from another terminal if the lock screen ever
fails.

Passwords are checked by PAM. For the lock screen's own service files:

```
sudo cp data/pam/gniri data/pam/gniri-fingerprint /etc/pam.d/
```

They are written for Fedora; the files say what to change elsewhere.
Without them gniri uses the services Fedora has for the same purpose
(`password-auth`, `fingerprint-auth`), or failing that `login`, which on
some systems waits for a fingerprint before it looks at a password.

gniri is also the session's polkit authentication agent (the dialog that
asks for a password when a program needs privileges). Stop any other agent:
polkit accepts one per session, and a second one gives up.

Settings go in `~/.config/gniri/config.toml`; `data/config.example.toml`
lists all of them. Without a `[wallpaper]` setting gniri draws no wallpaper.

## Documentation

- `docs/FIDELITY.md`: where gniri differs from GNOME Shell, and why.
- `docs/TESTING.md`: the test harness and the GNOME reference.
- `docs/DECISIONS.md`: design decisions and the experiments behind them.

## Licence

GPL-3.0-or-later. The theme is ported from gnome-shell (GPL-2.0-or-later) and
follows blur-my-shell (GPL-3.0).
