"""Phase-0 intrinsic-backed `tkinter.scrolledtext` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TK_CALL = _require_intrinsic("molt_tk_call", globals())


def _app_handle(widget):
    app = widget._tk_app
    return getattr(app, "_handle", app)


def _widget_call(widget, command, *argv):
    return _MOLT_TK_CALL(_app_handle(widget), [widget._w, command, *argv])


class ScrolledText(_tkinter.Widget):
    """Phase-0 text widget shell with `ScrolledText` API shape."""

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, "text", cnf, **kw)

    def insert(self, index, chars, *tags):
        return _widget_call(self, "insert", index, chars, *tags)

    def delete(self, index1, index2=None):
        if index2 is None:
            return _widget_call(self, "delete", index1)
        return _widget_call(self, "delete", index1, index2)

    def get(self, index1, index2=None):
        if index2 is None:
            return _widget_call(self, "get", index1)
        return _widget_call(self, "get", index1, index2)

    def yview(self, *args):
        return _widget_call(self, "yview", *args)

    def xview(self, *args):
        return _widget_call(self, "xview", *args)


Text = ScrolledText

__all__ = ["ScrolledText", "Text"]
