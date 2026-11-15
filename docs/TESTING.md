# Testing

```
cargo test --workspace          # logic, theme values, backends; no display needed
cargo xtask diff quick-settings # looks: gniri against the GNOME reference
cargo xtask check               # behaviour: gniri driven in a live session
cargo xtask report              # diff every component, write an HTML page
```

`diff`, `check` and `report` run gniri inside a nested niri window, so they
need a running Wayland session with the screen unlocked (a locked screen
stalls nested niri completely). You can keep working in other windows. They
also need ImageMagick (`magick`); `check` uses `wlrctl` for pointer input and
`wtype` for keys, and skips those checks without them.

Every run uses `tests/visual/scenes/config.toml`, never your own config, a
private D-Bus session bus, and a fixture in place of the system: no click in
a test turns your Wi-Fi off.

## Backends

`cargo test` also runs the backends against stand-ins for the system, and
skips each of these with a note when its tools are not installed:

- **UPower, NetworkManager, BlueZ**: python-dbusmock (`python3-dbusmock`) on
  a private bus (`crates/services/tests/*_bus.rs`).
- **Sound**: a private PipeWire with null devices
  (`crates/services/tests/audio_server.rs`); your own sound server is not
  touched.

To see what a backend reports from the real system, without changing
anything: `cargo run -p gniri-services --example watch -- power` (or
`network`, `bluetooth`, `brightness`, `audio`, `rfkill`).

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
4. `cargo xtask diff <name>`, then open the pictures under
   `target/visual/report/<name>/`: ours on top, GNOME in the middle,
   differing pixels marked at the bottom.

## Looking inside a running shell

```
cargo xtask nest --scene quick-settings --wallpaper '#808080'   # until Ctrl-C
gniri msg debug dump-tree        # every mapped widget
gniri msg debug commands         # what the shell asked its services for
gniri msg debug open-submenu wifi
```

`nest` prints the sockets of the nested session. With `GNIRI_SOCKET` set to
the `gniri_socket` path in `target/visual/logs/nest.ready`, `gniri msg`
reaches that shell. Without `--scene` the nested shell uses the real
services: what you click there happens.
