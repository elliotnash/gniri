# Configuring gniri

gniri has one file of settings:

```
~/.config/gniri/config.toml        ($XDG_CONFIG_HOME/gniri/config.toml)
```

Every setting is optional, and without the file gniri uses the defaults
given below. `data/config.example.toml` (installed as
`share/doc/gniri/config.example.toml`) has every setting commented out,
ready to copy:

```
mkdir -p ~/.config/gniri
cp data/config.example.toml ~/.config/gniri/config.toml
```

`gniri --config FILE` reads another file instead.

## Changes take effect at once

gniri watches the file. Save it and the change is there, with nothing to
restart: the clock, the wallpaper, the dash, the idle times and the rest
all follow. A menu that is open when its settings change closes.

A file with a mistake in it changes nothing. gniri keeps the settings it
had and says what is wrong in a notification, and on its standard error
(`journalctl --user -u gniri` when it runs as a service):

- a key gniri does not know is a mistake, not ignored, so a misspelt
  setting is reported instead of silently doing nothing;
- so is a value that makes no sense, such as a colour that is none.

At start, a file with a mistake means all defaults.

Two things are read only when a process starts. The lock screen is a
process of its own, started for each lock: it has the settings as they
are then. A wallpaper given on the command line (`--wallpaper`) wins over
the file's for as long as that gniri runs.

If the file is a symbolic link (as dotfile managers make them), gniri
follows it to the file it pointed to when gniri started.

## The settings

### `[wallpaper]`

| Key | Default | |
|---|---|---|
| `image` | none | An image file, scaled to cover each output, as GNOME's "zoom" does. `~/` is your home directory. Any format GTK reads, JPEG XL among them. |
| `color` | none | A flat colour, `"#rgb"` or `"#rrggbb"`, used when there is no image. |

With neither, gniri draws no wallpaper: use that if another program
draws yours. The overview's blurred background and the lock screen's are
made from gniri's wallpaper, so with none they are one flat, dark colour.

### `[clock]`

The clock in the middle of the top bar.

| Key | Default | |
|---|---|---|
| `format` | `"24h"` | Or `"12h"`. The lock screen's clock follows it. |
| `show_date` | `true` | The date before the time. |
| `show_weekday` | `false` | The day of the week before that. |
| `show_seconds` | `false` | |

### `[panel]`

| Key | Default | |
|---|---|---|
| `show_battery_percentage` | `false` | The charge as a number beside the battery icon. |
| `outputs` | every output | The outputs that get a top bar, by connector name: `["eDP-1"]`. `niri msg outputs` lists the names. |

The search entry and the dash in the overview belong with the bar: an
output without one shows niri's overview as it is.

### `[date_menu]`

The menu under the clock.

| Key | Default | |
|---|---|---|
| `week_start` | the locale's | The day a week starts on in the calendar: `"sunday"`, `"monday"` or `"saturday"`. Unset, it is the day the locale starts a week on. |
| `world_clocks` | `true` | The "Add World Clocks…" card. It only starts GNOME Clocks, and is hidden when that is not installed. |
| `weather` | `true` | The "Select Weather Location…" card, likewise for GNOME Weather. |

### `[night_light]`

| Key | Default | |
|---|---|---|
| `temperature` | `2700` | The colour temperature of the screen while Night Light is on, in kelvin, from 1700 to 4700. Lower is warmer. |

Night Light is on while its toggle in the quick settings is; there is no
schedule.

### `[idle]`

What happens when nobody touches the machine. A video or a call that asks
the compositor to stay awake holds all of it off.

| Key | Default | |
|---|---|---|
| `blank_after` | `300` | Seconds without input after which the screens turn off (GNOME's "Blank Screen Delay"). `0` for never. |
| `dim` | `true` | Dim the screen for the last fifth of that time. |

### `[lock]`

| Key | Default | |
|---|---|---|
| `automatic` | `true` | Lock when the screens turn off for idleness. |
| `delay` | `0` | Seconds between the screens turning off and the lock. |
| `on_suspend` | `true` | Lock before the machine sleeps. |

`gniri msg lock`, the lock button in the quick settings and
`loginctl lock-session` lock whatever these say.

### `[dash]`

| Key | Default | |
|---|---|---|
| `favorites` | GNOME's | The applications that are always in the dash, in order, by desktop entry: the name of a file in `/usr/share/applications` or `~/.local/share/applications`, with or without `.desktop`. One that is not installed is left out. |
| `icon_size` | as large as fits | The size of the dash's icons: 16, 22, 24, 32, 48 or 64. |

GNOME's favourites are Firefox, Calendar, Files, Software, Text Editor and
Calculator.

### `[search]`

| Key | Default | |
|---|---|---|
| `providers` | `true` | Ask the applications that offer search results (Files, Calculator, Characters, Software and so on) beside finding applications by name. |
| `disabled` | none | Applications that are not asked, by desktop entry: `["org.gnome.Software"]`. |

### `[commands]`

| Key | Default | |
|---|---|---|
| `settings` | `"gnome-control-center"` | The settings application, started as `<settings> <panel>` by entries such as "Power Settings" (the panels are `power`, `wifi`, `bluetooth` and `sound`). `""` hides those entries, as does a program that is not installed. |

## What is not in this file

- **Keys.** The volume, brightness, lock and app-grid keys are binds in
  niri's config. `gniri.kdl`, the fragment niri's config includes, has
  them; a bind of the same key after the `include` line replaces one.
- **Blur and the overview's zoom** are niri's too, and are in the same
  fragment.
- **Dark style** is the desktop-wide `color-scheme` setting that
  applications read (`gsettings get org.gnome.desktop.interface
  color-scheme`); the toggle in the quick settings sets it.
- **Do Not Disturb and Night Light** are toggles of the quick settings,
  and are off again when gniri starts.
- **The lock screen's password check** is PAM's: see "Locking, idleness
  and passwords" in the README.
