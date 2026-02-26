"""Phase-0 intrinsic-backed `tkinter.dialog` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TK_DIALOG_SHOW = _require_intrinsic("molt_tk_dialog_show", globals())

TclError = _tkinter.TclError
Widget = getattr(_tkinter, "Widget", object)
Button = getattr(_tkinter, "Button", object)
Pack = getattr(_tkinter, "Pack", object)
DIALOG_ICON = "questhead"


def _resolve_master(master):
    if master is None:
        return _tkinter._get_default_root()
    if not isinstance(master, _tkinter.Misc):
        raise TypeError("dialog master must be a tkinter widget or root")
    return master


def _app_handle(master):
    app = master._tk_app
    return getattr(app, "_handle", app)


class Dialog:
    """Thin wrapper over the Tcl `tk_dialog` command."""

    def __init__(
        self,
        master=None,
        title=None,
        text=None,
        bitmap=None,
        default=0,
        strings=(),
    ):
        self.master = master
        self.title = "" if title is None else str(title)
        self.text = "" if text is None else str(text)
        self.bitmap = DIALOG_ICON if bitmap is None else str(bitmap)
        self.default = int(default)
        self.strings = tuple(str(value) for value in strings)
        self.num = None

    def show(self):
        master = _resolve_master(self.master)
        result = _MOLT_TK_DIALOG_SHOW(
            _app_handle(master),
            str(master),
            self.title,
            self.text,
            self.bitmap,
            self.default,
            list(self.strings),
        )
        try:
            self.num = int(result)
        except Exception:  # noqa: BLE001
            self.num = result
        return self.num


__all__ = ["Dialog"]
