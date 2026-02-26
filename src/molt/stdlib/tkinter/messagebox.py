"""Phase-0 intrinsic-backed `tkinter.messagebox` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog

_MOLT_TK_MESSAGEBOX_SHOW = _require_intrinsic("molt_tk_messagebox_show", globals())

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
        master = _resolve_master(self.master)
        return _MOLT_TK_MESSAGEBOX_SHOW(
            _app_handle(master),
            str(master),
            _normalize_options(self.options),
        )


def _normalize_option_name(name):
    return name if name.startswith("-") else f"-{name}"


def _normalize_options(options):
    normalized = []
    for key, value in options.items():
        if value is None:
            continue
        option_name = _normalize_option_name(str(key))
        option_value = str(value) if option_name == "-parent" else value
        normalized.append(option_name)
        normalized.append(option_value)
    return normalized


def _resolve_master(master):
    if master is None:
        return _tkinter._get_default_root()
    if not isinstance(master, _tkinter.Misc):
        raise TypeError("messagebox master must be a tkinter widget or root")
    return master


def _app_handle(master):
    app = master._tk_app
    return getattr(app, "_handle", app)


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
