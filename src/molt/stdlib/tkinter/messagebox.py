"""Intrinsic-backed `tkinter.messagebox` wrappers."""

from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog

_MOLT_TK_MESSAGEBOX_SHOW = _require_intrinsic("molt_tk_messagebox_show")

Dialog = _commondialog.Dialog

ERROR = "error"
INFO = "info"
QUESTION = "question"
WARNING = "warning"

ABORTRETRYIGNORE = "abortretryignore"
OK = "ok"
OKCANCEL = "okcancel"
RETRYCANCEL = "retrycancel"
YESNO = "yesno"
YESNOCANCEL = "yesnocancel"

ABORT = "abort"
RETRY = "retry"
IGNORE = "ignore"
CANCEL = "cancel"
YES = "yes"
NO = "no"


class Message(_commondialog.Dialog):
    command = "tk_messageBox"

    def show(self, **options):
        if options:
            self.options.update(options)
        master = _commondialog._resolve_master(
            self.master,
            role="messagebox master",
        )
        return _MOLT_TK_MESSAGEBOX_SHOW(
            _commondialog._app_handle(master),
            str(master),
            _commondialog._prepare_intrinsic_options(self.options),
        )


def _show(title=None, message=None, _icon=None, _type=None, **options):
    if title is not None:
        options["title"] = title
    if message is not None:
        options["message"] = message
    if _icon is not None and "icon" not in options:
        options["icon"] = _icon
    if _type is not None and "type" not in options:
        options["type"] = _type
    result = Message(**options).show()
    if isinstance(result, bool):
        return YES if result else NO
    return str(result)


def showinfo(title=None, message=None, **options):
    return _show(title, message, INFO, OK, **options)


def showwarning(title=None, message=None, **options):
    return _show(title, message, WARNING, OK, **options)


def showerror(title=None, message=None, **options):
    return _show(title, message, ERROR, OK, **options)


def askquestion(title=None, message=None, **options):
    return _show(title, message, QUESTION, YESNO, **options)


def askokcancel(title=None, message=None, **options):
    return _show(title, message, QUESTION, OKCANCEL, **options) == OK


def askyesno(title=None, message=None, **options):
    return _show(title, message, QUESTION, YESNO, **options) == YES


def askyesnocancel(title=None, message=None, **options):
    result = _show(title, message, QUESTION, YESNOCANCEL, **options)
    if result == CANCEL:
        return None
    return result == YES


def askretrycancel(title=None, message=None, **options):
    return _show(title, message, WARNING, RETRYCANCEL, **options) == RETRY


__all__ = [
    "ABORT",
    "ABORTRETRYIGNORE",
    "CANCEL",
    "ERROR",
    "IGNORE",
    "INFO",
    "Dialog",
    "Message",
    "NO",
    "OK",
    "OKCANCEL",
    "QUESTION",
    "RETRY",
    "RETRYCANCEL",
    "WARNING",
    "YES",
    "YESNO",
    "YESNOCANCEL",
    "askokcancel",
    "askquestion",
    "askretrycancel",
    "askyesno",
    "askyesnocancel",
    "showerror",
    "showinfo",
    "showwarning",
]

globals().pop("_require_intrinsic", None)
