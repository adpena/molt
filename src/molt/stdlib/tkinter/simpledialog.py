"""Phase-0 intrinsic-backed `tkinter.simpledialog` wrappers."""

from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog
from tkinter import dialog as _dialog
from tkinter import messagebox as messagebox

_MOLT_TK_SIMPLEDIALOG_QUERY = _require_intrinsic(
    "molt_tk_simpledialog_query", globals()
)


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
    parent_widget = _commondialog._resolve_master(
        parent,
        role="simpledialog parent",
    )
    return _MOLT_TK_SIMPLEDIALOG_QUERY(
        _commondialog._app_handle(parent_widget),
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
        self.root = master
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

    def return_event(self, event):
        del event
        if self.default is None:
            bell = getattr(self.master, "bell", None)
            if callable(bell):
                bell()
            return None
        self.done(self.default)
        return None

    def wm_delete_window(self):
        if self.cancel is None:
            bell = getattr(self.master, "bell", None)
            if callable(bell):
                bell()
            return None
        self.done(self.cancel)
        return None

    def done(self, num):
        self.num = num
        return num


class Dialog:
    """Compatibility shell for the classic modal-dialog API."""

    def __init__(self, parent, title=None):
        self.parent = parent
        self.title = title
        self.initial_focus = None
        self.result = None

    def destroy(self):
        self.initial_focus = None
        return None

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
        self.destroy()
        return None


class _QueryDialog(Dialog):
    query_kind = "string"
    errormessage = "Illegal value."

    def __init__(
        self,
        title,
        prompt,
        initialvalue=None,
        minvalue=None,
        maxvalue=None,
        parent=None,
    ):
        self.prompt = prompt
        self.minvalue = minvalue
        self.maxvalue = maxvalue
        self.initialvalue = initialvalue
        self.entry = None
        super().__init__(parent, title)
        self.result = self._query_result()

    def _query_result(self):
        raw = _query(
            parent=self.parent,
            title=self.title,
            prompt=self.prompt,
            initialvalue=self.initialvalue,
            query_kind=self.query_kind,
            minvalue=self.minvalue,
            maxvalue=self.maxvalue,
        )
        if raw in (None, ""):
            return None
        return self._coerce_result(raw)

    def _coerce_result(self, value):
        return value

    def destroy(self):
        self.entry = None
        return super().destroy()

    def body(self, master):
        del master
        self.entry = {"value": self.initialvalue}
        return self.entry

    def getresult(self):
        return self.result

    def validate(self):
        try:
            result = self.getresult()
        except ValueError:
            return 0
        if result is None:
            self.result = None
            return 1
        if self.minvalue is not None and result < self.minvalue:
            return 0
        if self.maxvalue is not None and result > self.maxvalue:
            return 0
        self.result = result
        return 1


class _QueryInteger(_QueryDialog):
    query_kind = "int"
    errormessage = "Not an integer."

    def _coerce_result(self, value):
        return int(value)

    def getresult(self):
        return self.result


class _QueryFloat(_QueryDialog):
    query_kind = "float"
    errormessage = "Not a floating-point value."

    def _coerce_result(self, value):
        return float(value)

    def getresult(self):
        return self.result


class _QueryString(_QueryDialog):
    query_kind = "string"

    def __init__(self, *args, **kw):
        if "show" in kw:
            self.__show = kw["show"]
            del kw["show"]
        else:
            self.__show = None
        super().__init__(*args, **kw)

    def body(self, master):
        entry = super().body(master)
        if self.__show is not None and isinstance(entry, dict):
            entry["show"] = self.__show
        return entry

    def getresult(self):
        return self.result


def askstring(title, prompt, **kw):
    dialog = _QueryString(title, prompt, **kw)
    return dialog.result


def askinteger(title, prompt, **kw):
    dialog = _QueryInteger(title, prompt, **kw)
    return dialog.result


def askfloat(title, prompt, **kw):
    dialog = _QueryFloat(title, prompt, **kw)
    return dialog.result


__all__ = [
    "Dialog",
    "SimpleDialog",
    "_QueryDialog",
    "_QueryFloat",
    "_QueryInteger",
    "_QueryString",
    "askfloat",
    "askinteger",
    "askstring",
    "messagebox",
]
