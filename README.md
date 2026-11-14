# gniri

A desktop shell for the [niri](https://github.com/YaLTeR/niri) compositor that
looks like GNOME Shell 51 with the blur-my-shell extension, built with GTK 4
and libadwaita in Rust.

**Status: early.** The first milestone is done: the wallpaper, the blurred
overview backdrop and the top bar (workspace dots, clock, status icons,
keyboard layout, screen-sharing indicator). No menus, notifications or lock
screen yet, and the status icons have no real data until the next one.

## Build

Fedora:

```
sudo dnf install rust cargo gtk4-devel libadwaita-devel gtk4-layer-shell-devel glib2-devel
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
for every app) and places gniri's surfaces. Remove any other bar you start.

Settings go in `~/.config/gniri/config.toml`; `data/config.example.toml`
lists all of them. Without a `[wallpaper]` setting gniri draws no wallpaper.

## Documentation

- `docs/FIDELITY.md`: where gniri differs from GNOME Shell, and why.
- `docs/TESTING.md`: the test harness and the GNOME reference.
- `docs/DECISIONS.md`: design decisions and the experiments behind them.

## Licence

GPL-3.0-or-later. The theme is ported from gnome-shell (GPL-2.0-or-later) and
follows blur-my-shell (GPL-3.0).
