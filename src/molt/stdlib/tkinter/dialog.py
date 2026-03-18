"""Intrinsic-backed `tkinter.dialog` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog


def _lazy_intrinsic(name):
    def _call(*args, **kwargs):
        return _require_intrinsic(name, globals())(*args, **kwargs)

    return _call


_MOLT_TK_DIALOG_SHOW = _lazy_intrinsic("molt_tk_dialog_show")

TclError = _tkinter.TclError
Widget = getattr(_tkinter, "Widget", object)
Button = getattr(_tkinter, "Button", object)
Pack = getattr(_tkinter, "Pack", object)
DIALOG_ICON = "questhead"


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
        master = _commondialog._resolve_master(self.master, role="dialog master")
        result = _MOLT_TK_DIALOG_SHOW(
            _commondialog._app_handle(master),
            str(master),
            self.title,
            self.text,
            self.bitmap,
            self.default,
            list(self.strings),
        )
        if isinstance(result, int):
            self.num = result
        elif isinstance(result, str) and result.lstrip("-").isdigit():
            self.num = int(result)
        else:
            self.num = result
        return self.num

    def destroy(self):
        self.num = None
        return None


def _test():
    dialog = Dialog(
        None,
        title="File Modified",
        text=(
            'File "Python.h" has been modified since the last time it was saved. '
            "Do you want to save it before exiting the application."
        ),
        bitmap=DIALOG_ICON,
        default=0,
        strings=("Save File", "Discard Changes", "Return to Editor"),
    )
    print(dialog.num)


__all__ = ["Dialog"]
