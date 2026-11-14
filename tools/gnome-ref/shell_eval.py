"""Talk to a running GNOME Shell over D-Bus.

Needs `global.context.unsafe_mode = true` (Looking Glass) for Eval and the
Screenshot interface to accept calls from an arbitrary process.
"""

import json
from pathlib import Path

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

HERE = Path(__file__).resolve().parent


class ShellError(RuntimeError):
    pass


class Shell:
    def __init__(self):
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def _call(self, path, interface, method, args, timeout_ms=30000):
        return self.bus.call_sync(
            "org.gnome.Shell", path, interface, method, args, None,
            Gio.DBusCallFlags.NONE, timeout_ms, None,
        ).unpack()

    def eval(self, code):
        """Evaluate JS in the shell and return the decoded JSON result."""
        ok, result = self._call(
            "/org/gnome/Shell", "org.gnome.Shell", "Eval", GLib.Variant("(s)", (code,)))
        if not ok:
            if result == "":
                raise ShellError("Eval refused: unsafe mode is off")
            raise ShellError(result)
        return json.loads(result) if result else None

    def unsafe_mode(self):
        try:
            return bool(self.eval("global.context.unsafe_mode"))
        except ShellError:
            return False

    def dump(self, root_expr):
        """Dump the mapped actor tree under `root_expr` (see dump.js)."""
        code = (HERE / "dump.js").read_text().replace("__ROOT__", root_expr)
        return self.eval(code)

    def screenshot(self, filename, area=None):
        """Screenshot the whole stage, or `area` = (x, y, w, h) in stage coords."""
        filename = str(Path(filename).resolve())
        if area is None:
            ok, used = self._call(
                "/org/gnome/Shell/Screenshot", "org.gnome.Shell.Screenshot",
                "Screenshot", GLib.Variant("(bbs)", (False, False, filename)))
        else:
            x, y, w, h = (int(round(v)) for v in area)
            ok, used = self._call(
                "/org/gnome/Shell/Screenshot", "org.gnome.Shell.Screenshot",
                "ScreenshotArea", GLib.Variant("(iiiibs)", (x, y, w, h, False, filename)))
        if not ok:
            raise ShellError(f"screenshot failed: {filename}")
        return used
