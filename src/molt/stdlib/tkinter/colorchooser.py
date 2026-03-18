"""Intrinsic-backed `tkinter.colorchooser` wrappers."""

from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog


def _lazy_intrinsic(name):
    def _call(*args, **kwargs):
        return _require_intrinsic(name, globals())(*args, **kwargs)

    return _call


_MOLT_TK_COMMONDIALOG_SHOW = _lazy_intrinsic("molt_tk_commondialog_show")
_molt_tk_hex_to_rgb = _lazy_intrinsic("molt_tk_hex_to_rgb")

Dialog = _commondialog.Dialog


def _hex_to_rgb(color):
    return _molt_tk_hex_to_rgb(color)


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
