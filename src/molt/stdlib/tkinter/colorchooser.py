"""Phase-0 intrinsic-backed `tkinter.colorchooser` wrappers."""

from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog

_MOLT_TK_COMMONDIALOG_SHOW = _require_intrinsic("molt_tk_commondialog_show", globals())


def _hex_to_rgb(color):
    if not isinstance(color, str) or len(color) != 7 or not color.startswith("#"):
        return None
    try:
        return tuple(int(color[idx : idx + 2], 16) for idx in (1, 3, 5))
    except ValueError:
        return None


class Chooser(_commondialog.Dialog):
    command = "tk_chooseColor"

    def _fixresult(self, widget, result):
        del widget
        if not result:
            return (None, None)
        if isinstance(result, tuple) and len(result) == 2:
            return result
        if isinstance(result, str):
            return (_hex_to_rgb(result), result)
        return (None, result)

    def show(self, **options):
        if options:
            self.options.update(options)
        if not self.command:
            raise RuntimeError("dialog command is not configured")
        master = _commondialog._resolve_master(self.master)
        self._fixoptions()
        result = _MOLT_TK_COMMONDIALOG_SHOW(
            _commondialog._app_handle(master),
            str(master),
            self.command,
            _commondialog._normalize_options(self.options),
        )
        return self._fixresult(master, result)


def askcolor(color=None, **options):
    if color is not None and "initialcolor" not in options:
        options["initialcolor"] = color
    return Chooser(**options).show()


__all__ = ["Chooser", "askcolor"]
