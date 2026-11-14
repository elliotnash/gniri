#!/usr/bin/env python3
"""Capture reference metrics and screenshots from the running GNOME Shell.

Requires unsafe mode (Looking Glass: `global.context.unsafe_mode = true`).

For each UI state this opens the state by script, dumps the actor tree
(dump.js) and takes a full-screen screenshot, on three wallpapers: the user's
real one and two flat colours. Everything it changes is restored on exit:
wallpaper settings, animations, the background-logo extension, the unsafe-mode
panel icon and the active workspace.

Output goes to tests/visual/reference/gnome-<version>/raw/, which is
git-ignored: dumps contain label text (notifications, search results) and
screenshots show the desktop. Curated extracts are committed separately.

Usage: capture.py [--countdown SECONDS] [--only STATE[,...]] [--wallpapers NAME[,...]]
"""

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

from shell_eval import Shell, ShellError  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
LOGO_EXTENSION = "background-logo@fedorahosted.org"

QS = "Main.panel.statusArea.quickSettings"
DATE = "Main.panel.statusArea.dateMenu"
POLKIT = "Main.componentManager._allComponents.polkitAgent"

# True while the lock screen is up. Menus still open over it and screenshots
# still work, so a capture taken then looks plausible but shows the wrong UI.
IS_LOCKED = "Main.sessionMode.isLocked || Main.screenShield.locked"


class Locked(RuntimeError):
    """The session is locked: nothing captured now would be valid."""


# The Power Mode toggle: its submenu has fixed content, unlike the Wi-Fi and
# Bluetooth ones, which list nearby networks and devices.
QS_MENU_TOGGLE = (
    f"{QS}.menu._grid.get_children()"
    ".find(c => c.visible && c.menu && c.constructor.name === 'PowerProfilesToggle')"
)

WALLPAPERS = [
    ("real", None),
    ("flat-808080", "#808080"),
    ("flat-3584e4", "#3584e4"),
]

BACKGROUND_KEYS = [
    "picture-uri", "picture-uri-dark", "picture-options",
    "primary-color", "secondary-color", "color-shading-type",
]


def js(body):
    """Wrap statements so Eval returns the value of the last `return`."""
    return f"(() => {{ {body} }})()"


# GNOME keeps a banner on screen until the user is active again, so during a
# hands-off capture a banner never times out by itself. Remove our own
# announcement and retire (without destroying) anyone else's banner.
CLEAR_BANNERS = js(
    "for (const s of Main.messageTray.getSources().filter(s => s.title === 'gniri')) s.destroy();"
    "Main.messageTray._expireNotification();"
)


# name, JS to enter, JS to leave, seconds to settle after entering
STATES = [
    ("desktop", None, None, 0.2),
    (
        "panel-hover",
        js(f"for (const b of [Main.panel.statusArea.activities, {DATE}, {QS}]) "
           "b.add_style_pseudo_class('hover');"),
        js(f"for (const b of [Main.panel.statusArea.activities, {DATE}, {QS}]) "
           "b.remove_style_pseudo_class('hover');"),
        0.4,
    ),
    ("quick-settings", f"{QS}.menu.open()", f"{QS}.menu.close()", 0.7),
    (
        "quick-settings-submenu",
        js(f"{QS}.menu.open(); const t = {QS_MENU_TOGGLE}; t.menu.open(); "
           "return t.title ?? null;"),
        js(f"const t = {QS_MENU_TOGGLE}; t?.menu.close(); {QS}.menu.close();"),
        0.8,
    ),
    ("date-menu", f"{DATE}.menu.open()", f"{DATE}.menu.close()", 0.7),
    ("overview", "Main.overview.show()", "Main.overview.hide()", 1.0),
    ("app-grid", "Main.overview.showApps()", "Main.overview.hide()", 1.2),
    (
        "search",
        js("Main.overview.show(); Main.overview.searchEntry.set_text('calculator');"),
        js("Main.overview.searchEntry.set_text(''); Main.overview.hide();"),
        2.0,
    ),
    (
        "osd",
        js("Main.osdWindowManager.showAll("
           "Gio.ThemedIcon.new('audio-volume-high-symbolic'), null, 0.6, 1.0);"),
        None,
        0.3,
    ),
    (
        "notification-banner",
        "Main.notify('Reference notification', 'Body text of the reference notification')",
        js("Main.messageTray._notification?.destroy();"),
        0.8,
    ),
    ("run-dialog", "Main.openRunDialog()", "Main.runDialog?.close()", 0.7),
]


class Capture:
    def __init__(self, out_dir):
        self.shell = Shell()
        self.out = out_dir
        self.background = Gio.Settings.new("org.gnome.desktop.background")
        self.interface = Gio.Settings.new("org.gnome.desktop.interface")
        self.saved_background = {}
        self.saved_animations = None
        self.logo_was_enabled = False
        self.original_workspace = None
        self.idle_cookie = None

    # --- setup and restore -------------------------------------------------

    def check_unlocked(self):
        if self.shell.eval(IS_LOCKED):
            raise Locked("the screen is locked; unlock it and run the capture again")

    def inhibit_idle(self):
        """Stop the screen blanking and locking while nobody touches the machine."""
        (self.idle_cookie,) = self.shell.bus.call_sync(
            "org.gnome.SessionManager", "/org/gnome/SessionManager",
            "org.gnome.SessionManager", "Inhibit",
            GLib.Variant("(susu)", ("gniri-capture", 0, "Reference capture in progress", 8)),
            None, Gio.DBusCallFlags.NONE, 5000, None,
        ).unpack()

    def uninhibit_idle(self):
        if self.idle_cookie is not None:
            self.shell.bus.call_sync(
                "org.gnome.SessionManager", "/org/gnome/SessionManager",
                "org.gnome.SessionManager", "Uninhibit",
                GLib.Variant("(u)", (self.idle_cookie,)),
                None, Gio.DBusCallFlags.NONE, 5000, None,
            )
            self.idle_cookie = None

    def setup(self):
        self.check_unlocked()
        self.inhibit_idle()
        for key in BACKGROUND_KEYS:
            self.saved_background[key] = self.background.get_value(key)
        self.saved_animations = self.interface.get_boolean("enable-animations")
        self.interface.set_boolean("enable-animations", False)

        enabled = subprocess.run(
            ["gnome-extensions", "list", "--enabled"], capture_output=True, text=True
        ).stdout.split()
        self.logo_was_enabled = LOGO_EXTENSION in enabled
        if self.logo_was_enabled:
            subprocess.run(["gnome-extensions", "disable", LOGO_EXTENSION], check=False)

        self.shell.eval(CLEAR_BANNERS)

        # An empty workspace keeps other windows out of every screenshot.
        self.original_workspace = self.shell.eval(js(
            "const wm = global.workspace_manager;"
            "const original = wm.get_active_workspace_index();"
            "wm.get_workspace_by_index(wm.n_workspaces - 1).activate(global.get_current_time());"
            "return original;"
        ))
        # The unsafe-mode shield is not part of a normal panel.
        self.shell.eval(js(f"{QS}._unsafeMode._indicator.visible = false;"))
        # Park the pointer where it hovers nothing in any state.
        self.shell.eval(js(
            "global.stage.context.get_backend().get_default_seat()"
            ".warp_pointer(global.stage.width - 1, global.stage.height - 1);"
        ))
        time.sleep(1.0)

    def restore(self):
        problems = []

        def step(label, fn):
            try:
                fn()
            except Exception as error:  # keep restoring whatever else we can
                problems.append(f"{label}: {error}")

        step("close states", lambda: self.shell.eval(js(
            f"{QS}.menu.close(); {DATE}.menu.close(); Main.runDialog?.close();"
            "Main.overview.searchEntry.set_text(''); Main.overview.hide();"
        )))
        step("banners", lambda: self.shell.eval(CLEAR_BANNERS))
        step("polkit", lambda: self.shell.eval(f"{POLKIT}?._currentDialog?.cancel()"))
        step("unsafe icon", lambda: self.shell.eval(js(
            f"{QS}._unsafeMode._indicator.visible = global.context.unsafe_mode;")))
        if self.original_workspace is not None:
            step("workspace", lambda: self.shell.eval(js(
                "global.workspace_manager.get_workspace_by_index("
                f"{int(self.original_workspace)})?.activate(global.get_current_time());")))
        for key, value in self.saved_background.items():
            step(f"background {key}", lambda k=key, v=value: self.background.set_value(k, v))
        if self.saved_animations is not None:
            step("animations", lambda: self.interface.set_boolean(
                "enable-animations", self.saved_animations))
        if self.logo_was_enabled:
            step("logo extension", lambda: subprocess.run(
                ["gnome-extensions", "enable", LOGO_EXTENSION], check=True))
        step("idle inhibitor", self.uninhibit_idle)
        Gio.Settings.sync()
        return problems

    # --- capture -----------------------------------------------------------

    def set_wallpaper(self, colour):
        if colour is None:
            for key, value in self.saved_background.items():
                self.background.set_value(key, value)
        else:
            self.background.set_string("picture-options", "none")
            self.background.set_string("color-shading-type", "solid")
            self.background.set_string("primary-color", colour)
            self.background.set_string("secondary-color", colour)
        Gio.Settings.sync()
        time.sleep(2.5)

    def shoot(self, wallpaper, name, dump):
        shot_dir = self.out / wallpaper
        shot_dir.mkdir(parents=True, exist_ok=True)
        if dump:
            metrics_dir = self.out / "metrics"
            metrics_dir.mkdir(parents=True, exist_ok=True)
            tree = self.shell.dump("Main.layoutManager.uiGroup")
            (metrics_dir / f"{name}.json").write_text(json.dumps(tree, indent=1))
        self.shell.screenshot(shot_dir / f"{name}.png")

    def run_state(self, wallpaper, state, dump):
        name, enter, leave, settle = state
        note = None
        self.check_unlocked()
        # Every state starts from the plain desktop. Something else (a stray
        # Super press, a hot corner) may have opened the overview meanwhile.
        if self.shell.eval(js(
                "if (!Main.overview.visible) return false; Main.overview.hide(); return true;")):
            print(f"  overview was open before {name}; closed it", file=sys.stderr)
            time.sleep(1.0)
        try:
            if enter:
                note = self.shell.eval(enter)
            time.sleep(settle)
            self.check_unlocked()
            self.shoot(wallpaper, name, dump)
        finally:
            if leave:
                try:
                    self.shell.eval(leave)
                except ShellError as error:
                    print(f"  leave {name}: {error}", file=sys.stderr)
            time.sleep(0.4 if leave else 2.2)  # the OSD times out by itself
        return note

    def run_polkit(self, wallpaper, dump):
        """The polkit dialog needs a real request; cancel it straight after."""
        process = subprocess.Popen(
            ["pkexec", "true"], stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(30):
                if self.shell.eval(f"!!{POLKIT}?._currentDialog"):
                    break
                time.sleep(0.1)
            else:
                return "polkit dialog did not appear"
            time.sleep(0.7)
            self.check_unlocked()
            self.shoot(wallpaper, "polkit-dialog", dump)
            return None
        finally:
            try:
                self.shell.eval(f"{POLKIT}?._currentDialog?.cancel()")
            except ShellError as error:
                print(f"  polkit cancel: {error}", file=sys.stderr)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
            time.sleep(0.4)

    def run(self, only, wallpapers):
        notes = {}
        for wallpaper, colour in WALLPAPERS:
            if wallpapers and wallpaper not in wallpapers:
                continue
            self.set_wallpaper(colour)
            dump = wallpaper == "real"  # metrics do not depend on the wallpaper
            for state in STATES:
                if only and state[0] not in only:
                    continue
                print(f"{wallpaper}: {state[0]}", flush=True)
                try:
                    note = self.run_state(wallpaper, state, dump)
                    if note is not None:
                        notes[state[0]] = note
                except ShellError as error:
                    notes[f"{wallpaper}/{state[0]}"] = f"FAILED: {error}"
                    print(f"  FAILED: {error}", file=sys.stderr)
            if not only or "polkit-dialog" in only:
                print(f"{wallpaper}: polkit-dialog", flush=True)
                try:
                    problem = self.run_polkit(wallpaper, dump)
                    if problem:
                        notes[f"{wallpaper}/polkit-dialog"] = problem
                except ShellError as error:
                    notes[f"{wallpaper}/polkit-dialog"] = f"FAILED: {error}"
        return notes


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--countdown", type=int, default=8)
    parser.add_argument("--only", default="")
    parser.add_argument("--wallpapers", default="")
    args = parser.parse_args()
    only = {name for name in args.only.split(",") if name}
    wallpapers = {name for name in args.wallpapers.split(",") if name}

    shell = Shell()
    if not shell.unsafe_mode():
        sys.exit("GNOME Shell unsafe mode is off. In Looking Glass (Alt+F2, lg) run:\n"
                 "  global.context.unsafe_mode = true")

    version = shell.eval("Config.PACKAGE_VERSION")
    out_dir = REPO / "tests/visual/reference" / f"gnome-{version}" / "raw"
    out_dir.mkdir(parents=True, exist_ok=True)

    if args.countdown > 0:
        subprocess.run([
            "notify-send", "-a", "gniri", "gniri reference capture",
            f"Starting in {args.countdown} seconds. Hands off for about two minutes.",
        ], check=False)
        time.sleep(args.countdown)

    capture = Capture(out_dir)
    notes = {}
    aborted = None
    try:
        capture.setup()
        notes = capture.run(only, wallpapers)
    except Locked as error:
        aborted = str(error)
    finally:
        problems = capture.restore()

    meta = {
        "captured_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "gnome_shell": version,
        "extensions": subprocess.run(
            ["gnome-extensions", "list", "--enabled"], capture_output=True, text=True
        ).stdout.split(),
        "animations_disabled_during_capture": True,
        "notes": notes,
        "restore_problems": problems,
    }
    (out_dir / "meta.json").write_text(json.dumps(meta, indent=1))
    print(json.dumps({"notes": notes, "restore_problems": problems}, indent=1))
    if aborted:
        sys.exit(f"ABORTED: {aborted}. Shots taken before this point may be incomplete.")
    if problems:
        sys.exit(1)


if __name__ == "__main__":
    main()
