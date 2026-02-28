"""Intrinsic-backed `tkinter.colorchooser` wrappers."""

from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog

_MOLT_TK_COMMONDIALOG_SHOW = _require_intrinsic("molt_tk_commondialog_show", globals())

Dialog = _commondialog.Dialog


def _hex_to_rgb(color):
    if not isinstance(color, str) or len(color) != 7 or not color.startswith("#"):
        return None
    hex_part = color[1:]
    if not all(c in "0123456789abcdefABCDEF" for c in hex_part):
        return None
    return (int(color[1:3], 16), int(color[3:5], 16), int(color[5:7], 16))


class Chooser(_commondialog.Dialog):
    command = "tk_chooseColor"

    def _fixoptions(self):
        color = self.options.get("initialcolor")
        if isinstance(color, tuple) and len(color) == 3:
            self.options["initialcolor"] = "#%02x%02x%02x" % color

    def _fixresult(self, widget, result):
        if not result or not str(result):
            return (None, None)
        if isinstance(result, tuple) and len(result) == 2:
            return result
        winfo_rgb = getattr(widget, "winfo_rgb", None)
        if callable(winfo_rgb):
            red, green, blue = winfo_rgb(result)
            return ((red // 256, green // 256, blue // 256), str(result))
        return (_hex_to_rgb(str(result)), str(result))


def askcolor(color=None, **options):
    if color is not None and "initialcolor" not in options:
        options["initialcolor"] = color
    return Chooser(**options).show()


__all__ = ["Chooser", "Dialog", "askcolor"]
