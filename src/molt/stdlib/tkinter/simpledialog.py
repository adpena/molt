"""Phase-0 intrinsic-backed `tkinter.simpledialog` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import dialog as _dialog

_MOLT_TK_SIMPLEDIALOG_QUERY = _require_intrinsic(
    "molt_tk_simpledialog_query", globals()
)


def _resolve_parent(parent):
    if parent is None:
        return _tkinter._get_default_root()
    if not isinstance(parent, _tkinter.Misc):
        raise TypeError("simpledialog parent must be a tkinter widget or root")
    return parent


def _app_handle(widget):
    app = widget._tk_app
    return getattr(app, "_handle", app)


def _query(
    *,
    parent,
    title,
    prompt,
    initialvalue,
    query_kind,
    minvalue=None,
    maxvalue=None,
):
    parent_widget = _resolve_parent(parent)
    return _MOLT_TK_SIMPLEDIALOG_QUERY(
        _app_handle(parent_widget),
        str(parent_widget),
        "" if title is None else str(title),
        str(prompt),
        "" if initialvalue is None else str(initialvalue),
        query_kind,
        minvalue,
        maxvalue,
    )


class SimpleDialog:
    """Thin wrapper around `tkinter.dialog.Dialog` button dialogs."""

    def __init__(
        self,
        master,
        text="",
        buttons=(),
        default=None,
        cancel=None,
        title=None,
        class_=None,
    ):
        del class_
        self.master = master
        self.text = text
        self.buttons = tuple(buttons) if buttons else ("OK",)
        self.default = 0 if default is None else int(default)
        self.cancel = cancel
        self.title = title or ""
        self.num = None

    def go(self):
        dialog = _dialog.Dialog(
            master=self.master,
            title=self.title,
            text=self.text,
            default=self.default,
            strings=self.buttons,
        )
        self.num = dialog.show()
        if self.cancel is not None and self.num == self.cancel:
            return None
        return self.num


class Dialog:
    """Compatibility shell for the classic modal-dialog API."""

    def __init__(self, parent, title=None):
        self.parent = parent
        self.title = title
        self.result = None

    def body(self, master):
        del master
        return None

    def buttonbox(self):
        return None

    def validate(self):
        return True

    def apply(self):
        return None

    def ok(self, event=None):
        del event
        if self.validate():
            self.apply()
            return True
        return False

    def cancel(self, event=None):
        del event
        self.result = None
        return None


def askstring(title, prompt, **kw):
    parent = kw.pop("parent", None)
    initialvalue = kw.pop("initialvalue", "")
    return _query(
        parent=parent,
        title=title,
        prompt=prompt,
        initialvalue=initialvalue,
        query_kind="string",
    )


def askinteger(title, prompt, **kw):
    parent = kw.pop("parent", None)
    initialvalue = kw.pop("initialvalue", "")
    minvalue = kw.pop("minvalue", None)
    maxvalue = kw.pop("maxvalue", None)
    return _query(
        parent=parent,
        title=title,
        prompt=prompt,
        initialvalue=initialvalue,
        query_kind="int",
        minvalue=minvalue,
        maxvalue=maxvalue,
    )


def askfloat(title, prompt, **kw):
    parent = kw.pop("parent", None)
    initialvalue = kw.pop("initialvalue", "")
    minvalue = kw.pop("minvalue", None)
    maxvalue = kw.pop("maxvalue", None)
    return _query(
        parent=parent,
        title=title,
        prompt=prompt,
        initialvalue=initialvalue,
        query_kind="float",
        minvalue=minvalue,
        maxvalue=maxvalue,
    )


__all__ = [
    "Dialog",
    "SimpleDialog",
    "askfloat",
    "askinteger",
    "askstring",
]
