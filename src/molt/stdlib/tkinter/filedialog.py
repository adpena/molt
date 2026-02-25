"""Phase-0 intrinsic-backed `tkinter.filedialog` wrappers."""

from _intrinsics import require_intrinsic as _require_intrinsic
from tkinter import commondialog as _commondialog

_MOLT_TK_COMMONDIALOG_SHOW = _require_intrinsic("molt_tk_commondialog_show", globals())


class _Dialog(_commondialog.Dialog):
    command = ""

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


class Open(_Dialog):
    command = "tk_getOpenFile"

    def _fixresult(self, widget, result):
        if self.options.get("multiple"):
            if not result:
                return ()
            if isinstance(result, (tuple, list)):
                return tuple(result)
            return tuple(widget.splitlist(result))
        return result


class SaveAs(_Dialog):
    command = "tk_getSaveFile"


class Directory(_Dialog):
    command = "tk_chooseDirectory"


def askopenfilename(**options):
    return Open(**options).show()


def asksaveasfilename(**options):
    return SaveAs(**options).show()


def askopenfilenames(**options):
    options["multiple"] = True
    return Open(**options).show()


def askdirectory(**options):
    return Directory(**options).show()


def askopenfile(mode="r", **options):
    filename = askopenfilename(**options)
    if not filename:
        return None
    return open(filename, mode)


def askopenfiles(mode="r", **options):
    files = []
    for filename in askopenfilenames(**options):
        files.append(open(filename, mode))
    return files


def asksaveasfile(mode="w", **options):
    filename = asksaveasfilename(**options)
    if not filename:
        return None
    return open(filename, mode)


__all__ = [
    "Directory",
    "Open",
    "SaveAs",
    "askdirectory",
    "askopenfile",
    "askopenfilename",
    "askopenfiles",
    "askopenfilenames",
    "asksaveasfile",
    "asksaveasfilename",
]
