# gniri

A desktop shell for the [niri](https://github.com/YaLTeR/niri) compositor that
looks like GNOME Shell 51 with the blur-my-shell extension, built with GTK 4
and libadwaita in Rust.

**Status: early.** Two milestones are done:

- the wallpaper, the blurred overview backdrop and the top bar (workspace
  dots, clock, status icons, keyboard layout, screen-sharing indicator);
- the quick settings menu with its submenus, the volume and brightness
  display, and the services behind them: sound, Wi-Fi and wired networks,
  Bluetooth, battery and power mode, brightness, airplane mode, dark style
  and night light.

No date menu, notifications, lock screen or dialogs yet.

## Build

Fedora:

```
sudo dnf install rust cargo gtk4-devel libadwaita-devel gtk4-layer-shell-devel \
    glib2-devel pulseaudio-libs-devel
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
for every app), places gniri's surfaces, and binds the volume and brightness
keys to `gniri msg volume up|down|mute` and `gniri msg brightness up|down`.
Those binds expect `gniri` on your `PATH`, and only win over binds of the
same keys that come before the `include` line. Remove any other bar you
start.

gniri talks to the system directly: PipeWire or PulseAudio for sound,
NetworkManager, BlueZ, UPower, a power-profiles daemon and logind. Whatever
is missing is left out of the menu. It needs no gnome-settings-daemon.

Settings go in `~/.config/gniri/config.toml`; `data/config.example.toml`
lists all of them. Without a `[wallpaper]` setting gniri draws no wallpaper.

## Documentation

- `docs/FIDELITY.md`: where gniri differs from GNOME Shell, and why.
- `docs/TESTING.md`: the test harness and the GNOME reference.
- `docs/DECISIONS.md`: design decisions and the experiments behind them.

## Licence

GPL-3.0-or-later. The theme is ported from gnome-shell (GPL-2.0-or-later) and
follows blur-my-shell (GPL-3.0).
