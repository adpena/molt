"""Phase-0 intrinsic-backed `tkinter.tix` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TK_CALL = _require_intrinsic("molt_tk_call", globals())


def _resolve_master(master):
    if master is None:
        return _tkinter._get_default_root()
    if not isinstance(master, _tkinter.Misc):
        raise TypeError("tix master must be a tkinter widget or root")
    return master


def _app_handle(master):
    app = master._tk_app
    return getattr(app, "_handle", app)


class TixError(_tkinter.TclError):
    """Tix compatibility error type."""


class Tk(_tkinter.Tk):
    """Tix-flavored root wrapper."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)

    def tix_command(self, *args):
        return tixCommand(self, *args)


class TixWidget(_tkinter.Widget):
    """Minimal Tix widget shell backed by Tk calls."""

    _widget_command = "tixWidget"

    def __init__(self, master=None, widget_command=None, cnf=None, **kw):
        command = self._widget_command if widget_command is None else widget_command
        super().__init__(master, command, cnf, **kw)


class Balloon(TixWidget):
    _widget_command = "tixBalloon"


class ButtonBox(TixWidget):
    _widget_command = "tixButtonBox"


class ComboBox(TixWidget):
    _widget_command = "tixComboBox"


def tixCommand(master=None, *args):
    root = _resolve_master(master)
    return _MOLT_TK_CALL(_app_handle(root), ["tix", *args])


__all__ = [
    "Balloon",
    "ButtonBox",
    "ComboBox",
    "Tk",
    "TixError",
    "TixWidget",
    "tixCommand",
]
