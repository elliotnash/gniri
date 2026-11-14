# Testing

```
cargo test --workspace     # logic and theme values; no display needed
cargo xtask diff panel     # looks: gniri against the GNOME reference
cargo xtask check          # behaviour: gniri driven in a live session
cargo xtask report         # diff every component, write an HTML page
```

`diff`, `check` and `report` run gniri inside a nested niri window, so they
need a running Wayland session with the screen unlocked (a locked screen
stalls nested niri completely). You can keep working in other windows. They
also need ImageMagick (`magick`); `check` uses `wlrctl` for pointer input and
skips those checks without it.

Every run uses `tests/visual/scenes/config.toml`, never your own config, and
a private D-Bus session bus.

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
1792x1120 at the same scale. Nothing is resized. Each widget is compared
relative to the edge it is attached to (left, centre or right), and for the
real wallpaper the nested output is given the part of the wallpaper that lies
under that edge on the reference screen.

## Adding a component

1. Add it to `EXTRACTS` in `tools/gnome-ref/extract.py`, run the script,
   look at the files it wrote and commit them.
2. Write a scene, `tests/visual/scenes/<name>.toml`: the fixture (frozen
   clock, service state) and the steps that bring niri to the state the
   reference shows. The format is described in `crates/xtask/src/scene.rs`.
3. Write the map, `tests/visual/map/<name>.toml`: which widget is which
   actor, and which strips to compare. See `panel.toml`; selectors are
   described in `crates/xtask/src/tree.rs`.
4. `cargo xtask diff <name>`, then open the pictures under
   `target/visual/report/<name>/`: ours on top, GNOME in the middle,
   differing pixels marked at the bottom.

## Looking inside a running shell

```
cargo xtask nest --scene panel --wallpaper '#808080'   # until Ctrl-C
gniri msg debug dump-tree                              # every mapped widget
```

`nest` prints the sockets of the nested session. With `GNIRI_SOCKET` set to
the path in `target/visual/logs/nest.ready`, `gniri msg` reaches that shell.
