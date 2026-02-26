"""Intrinsic-backed `tkinter.tix` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TK_CALL = _require_intrinsic("molt_tk_call", globals())

# CPython-compatible symbolic constants.
WINDOW = "window"
TEXT = "text"
STATUS = "status"
IMMEDIATE = "immediate"
IMAGE = "image"
IMAGETEXT = "imagetext"
BALLOON = "balloon"
AUTO = "auto"
ACROSSTOP = "acrosstop"

ASCII = "ascii"
CELL = "cell"
COLUMN = "column"
DECREASING = "decreasing"
INCREASING = "increasing"
INTEGER = "integer"
MAIN = "main"
MAX = "max"
REAL = "real"
ROW = "row"
S_REGION = "s-region"
X_REGION = "x-region"
Y_REGION = "y-region"

TCL_DONT_WAIT = 1 << 1
TCL_WINDOW_EVENTS = 1 << 2
TCL_FILE_EVENTS = 1 << 3
TCL_TIMER_EVENTS = 1 << 4
TCL_IDLE_EVENTS = 1 << 5
TCL_ALL_EVENTS = 0


def _resolve_master(master):
    if master is None:
        return _tkinter._get_default_root()
    if not isinstance(master, _tkinter.Misc):
        raise TypeError("tix master must be a tkinter widget or root")
    return master


def _app_handle(master):
    app = master._tk_app
    return getattr(app, "_handle", app)


def _normalize_option_name(name):
    value = str(name)
    if value.startswith("-"):
        return value
    return f"-{value}"


def _normalize_options(cnf=None, **kw):
    if cnf is not None and not isinstance(cnf, dict):
        raise TypeError("tix config must be a dict or None")
    merged = {}
    if cnf:
        merged.update(cnf)
    if kw:
        merged.update(kw)
    normalized = []
    for key, value in merged.items():
        if value is None:
            continue
        normalized.append(_normalize_option_name(key))
        normalized.append(value)
    return normalized


class TixError(_tkinter.TclError):
    """Tix compatibility error type."""


class Tk(_tkinter.Tk):
    """Tix-flavored root wrapper."""

    def tix_command(self, *args):
        return self.tk.call("tix", *args)


class TixWidget(_tkinter.Widget):
    """Tix widget shell backed by Tk calls."""

    _widget_command = None

    def __init__(self, master=None, widget_command=None, cnf=None, **kw):
        command = widget_command or self._widget_command
        if not command:
            command = f"tix{self.__class__.__name__}"
        super().__init__(master, command, cnf, **kw)

    def subwidget(self, name):
        return TixSubWidget(self, name)


class TixSubWidget(TixWidget):
    """Lightweight wrapper around an existing Tix subwidget path."""

    def __init__(self, master, name):
        if not isinstance(master, _tkinter.Misc):
            raise TypeError("TixSubWidget master must be a tkinter widget")
        self.master = master
        self.tk = master.tk
        self._tk_app = master._tk_app
        self.children = {}
        try:
            path = master.call(master._w, "subwidget", name)
        except Exception:  # noqa: BLE001
            path = f"{master._w}.{name}"
        self._w = str(path)


class DisplayStyle:
    """DisplayStyle compatibility shell."""

    def __init__(self, itemtype=TEXT, refwindow=None, **kw):
        self.itemtype = itemtype
        self.refwindow = refwindow
        self._config = dict(kw)

    def config(self, cnf=None, **kw):
        if isinstance(cnf, str):
            return self._config.get(cnf)
        if cnf:
            self._config.update(cnf)
        if kw:
            self._config.update(kw)
        return None

    configure = config

    def cget(self, key):
        return self._config.get(key)

    def delete(self):
        self._config.clear()
        return None


class OptionName(str):
    """Option name marker used by legacy Tix wrappers."""


class FileTypeList(list):
    """Compatibility list container for file dialog type declarations."""


class InputOnly:
    """Marker mixin used by legacy Tix wrappers."""


class Control(TixWidget):
    _widget_command = "tixControl"


class ComboBox(TixWidget):
    _widget_command = "tixComboBox"


class DirList(TixWidget):
    _widget_command = "tixDirList"


class DirTree(TixWidget):
    _widget_command = "tixDirTree"


class DirSelectBox(TixWidget):
    _widget_command = "tixDirSelectBox"


class ExFileSelectBox(TixWidget):
    _widget_command = "tixExFileSelectBox"


class DirSelectDialog(TixWidget):
    _widget_command = "tixDirSelectDialog"


class ExFileSelectDialog(TixWidget):
    _widget_command = "tixExFileSelectDialog"


class FileSelectBox(TixWidget):
    _widget_command = "tixFileSelectBox"


class FileSelectDialog(TixWidget):
    _widget_command = "tixFileSelectDialog"


class FileEntry(TixWidget):
    _widget_command = "tixFileEntry"


class HList(TixWidget):
    _widget_command = "tixHList"


class CheckList(TixWidget):
    _widget_command = "tixCheckList"


class TList(TixWidget):
    _widget_command = "tixTList"


class Tree(TixWidget):
    _widget_command = "tixTree"


class NoteBook(TixWidget):
    _widget_command = "tixNoteBook"


class NoteBookFrame(TixWidget):
    _widget_command = "tixNoteBookFrame"


class PanedWindow(TixWidget):
    _widget_command = "tixPanedWindow"


class ListNoteBook(TixWidget):
    _widget_command = "tixListNoteBook"


class Meter(TixWidget):
    _widget_command = "tixMeter"


class ItemizedWidget(TixWidget):
    _widget_command = "tixItemizedWidget"


class ScrolledListBox(TixWidget):
    _widget_command = "tixScrolledListBox"


class ScrolledText(TixWidget):
    _widget_command = "tixScrolledText"


class ScrolledTList(TixWidget):
    _widget_command = "tixScrolledTList"


class ScrolledWindow(TixWidget):
    _widget_command = "tixScrolledWindow"


class ScrolledHList(TixWidget):
    _widget_command = "tixScrolledHList"


class ScrolledGrid(TixWidget):
    _widget_command = "tixScrolledGrid"


class StdButtonBox(TixWidget):
    _widget_command = "tixStdButtonBox"


class ButtonBox(TixWidget):
    _widget_command = "tixButtonBox"


class Balloon(TixWidget):
    _widget_command = "tixBalloon"


class ResizeHandle(TixWidget):
    _widget_command = "tixResizeHandle"


class LabelEntry(TixWidget):
    _widget_command = "tixLabelEntry"


class LabelFrame(TixWidget):
    _widget_command = "tixLabelFrame"


class MainWindow(TixWidget):
    _widget_command = "tixMainWindow"


class Select(TixWidget):
    _widget_command = "tixSelect"


class Shell(TixWidget):
    _widget_command = "tixShell"


class DialogShell(TixWidget):
    _widget_command = "tixDialogShell"


class PopupMenu(TixWidget):
    _widget_command = "tixPopupMenu"


class OptionMenu(TixWidget):
    _widget_command = "tixOptionMenu"


class CObjView(TixWidget):
    _widget_command = "tixCObjView"


class Grid(TixWidget):
    _widget_command = "tixGrid"


class Form(TixWidget):
    _widget_command = "tixForm"


def tixCommand(master=None, *args):
    root = _resolve_master(master)
    return _MOLT_TK_CALL(_app_handle(root), ["tix", *args])


__all__ = [
    "ACROSSTOP",
    "ASCII",
    "AUTO",
    "BALLOON",
    "Balloon",
    "ButtonBox",
    "CELL",
    "COLUMN",
    "CObjView",
    "CheckList",
    "ComboBox",
    "Control",
    "DECREASING",
    "DialogShell",
    "DirList",
    "DirSelectBox",
    "DirSelectDialog",
    "DirTree",
    "DisplayStyle",
    "ExFileSelectBox",
    "ExFileSelectDialog",
    "FileEntry",
    "FileSelectBox",
    "FileSelectDialog",
    "FileTypeList",
    "Form",
    "Grid",
    "HList",
    "IMAGE",
    "IMAGETEXT",
    "IMMEDIATE",
    "INCREASING",
    "INTEGER",
    "InputOnly",
    "LabelEntry",
    "LabelFrame",
    "ListNoteBook",
    "MAIN",
    "MAX",
    "Meter",
    "NoteBook",
    "NoteBookFrame",
    "OptionMenu",
    "OptionName",
    "PanedWindow",
    "PopupMenu",
    "REAL",
    "ROW",
    "ResizeHandle",
    "STATUS",
    "S_REGION",
    "ScrolledGrid",
    "ScrolledHList",
    "ScrolledListBox",
    "ScrolledTList",
    "ScrolledText",
    "ScrolledWindow",
    "Select",
    "Shell",
    "StdButtonBox",
    "TCL_ALL_EVENTS",
    "TCL_DONT_WAIT",
    "TCL_FILE_EVENTS",
    "TCL_IDLE_EVENTS",
    "TCL_TIMER_EVENTS",
    "TCL_WINDOW_EVENTS",
    "TEXT",
    "TList",
    "TixError",
    "TixSubWidget",
    "TixWidget",
    "Tk",
    "Tree",
    "WINDOW",
    "X_REGION",
    "Y_REGION",
    "tixCommand",
]
