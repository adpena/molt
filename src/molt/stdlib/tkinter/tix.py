"""Intrinsic-backed `tkinter.tix` wrappers."""

import warnings

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

try:
    warnings.warn(
        "tkinter.tix is deprecated since Python 3.6 and will be removed in Python 3.13. Use tkinter.ttk instead.",
        DeprecationWarning,
        stacklevel=2,
    )
except RuntimeError as exc:
    if "intrinsic unavailable" not in str(exc):
        raise


def _lazy_intrinsic(name):
    def _call(*args, **kwargs):
        return _require_intrinsic(name, globals())(*args, **kwargs)

    return _call


_MOLT_TK_CALL = _lazy_intrinsic("molt_tk_call")

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


def _options_tuple(cnf=None, **kw):
    return tuple(_normalize_options(cnf, **kw))


def _widget_path(widget):
    return getattr(widget, "_w", widget)


def _split_ints(widget, value):
    try:
        parts = widget.tk.splitlist(value)
    except Exception:  # noqa: BLE001
        return ()
    return tuple(widget.getint(part) for part in parts)


class tixCommand:
    """CPython-compatible Tix command mixin."""

    def __new__(cls, master=None, *args):
        # Preserve the legacy module-level callable shape: tixCommand(master, ...).
        if cls is tixCommand:
            root = _resolve_master(master)
            return _MOLT_TK_CALL(_app_handle(root), ["tix", *args])
        return super().__new__(cls)

    def tix_addbitmapdir(self, directory):
        return self.tk.call("tix", "addbitmapdir", directory)

    def tix_cget(self, option):
        return self.tk.call("tix", "cget", option)

    def tix_configure(self, cnf=None, **kw):
        if cnf is None and not kw:
            return self.tk.call("tix", "configure")
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "tix_configure option query cannot be combined with updates"
                )
            return self.tk.call("tix", "configure", _normalize_option_name(cnf))
        return self.tk.call("tix", "configure", *_options_tuple(cnf, **kw))

    def tix_filedialog(self, dlgclass=None):
        if dlgclass is None:
            return self.tk.call("tix", "filedialog")
        return self.tk.call("tix", "filedialog", dlgclass)

    def tix_getbitmap(self, name):
        return self.tk.call("tix", "getbitmap", name)

    def tix_getimage(self, name):
        return self.tk.call("tix", "getimage", name)

    def tix_option_get(self, name):
        return self.tk.call("tix", "option", "get", name)

    def tix_resetoptions(self, newScheme, newFontSet, newScmPrio=None):
        if newScmPrio is None:
            return self.tk.call("tix", "resetoptions", newScheme, newFontSet)
        return self.tk.call("tix", "resetoptions", newScheme, newFontSet, newScmPrio)


class TixError(_tkinter.TclError):
    """Tix compatibility error type."""


class Tk(_tkinter.Tk, tixCommand):
    """Tix-flavored root wrapper."""

    def tix_command(self, *args):
        return self.tk.call("tix", *args)

    def destroy(self):
        return super().destroy()


class TixWidget(_tkinter.Widget):
    """Tix widget shell backed by Tk calls."""

    _widget_command = None

    def __init__(self, master=None, widget_command=None, cnf=None, **kw):
        command = widget_command or self._widget_command
        if not command:
            command = f"tix{self.__class__.__name__}"
        super().__init__(master, command, cnf, **kw)
        self.subwidget_list = {}

    def __getattr__(self, name):
        subwidgets = self.__dict__.get("subwidget_list")
        if isinstance(subwidgets, dict) and name in subwidgets:
            return subwidgets[name]
        raise AttributeError(name)

    def _tix_call(self, *args):
        return self._call_widget(*args)

    def set_silent(self, value):
        return self.tk.call("tixSetSilent", self._w, value)

    def subwidget(self, name):
        key = str(name)
        subwidgets = self.__dict__.setdefault("subwidget_list", {})
        if key in subwidgets:
            return subwidgets[key]
        child = TixSubWidget(self, key)
        subwidgets[key] = child
        return child

    def subwidgets_all(self):
        names = self._subwidget_names()
        if not names:
            return []
        return [self.subwidget(name) for name in names]

    def _subwidget_name(self, name):
        try:
            return self._tix_call("subwidget", name)
        except Exception:  # noqa: BLE001
            return None

    def _subwidget_names(self):
        try:
            names = self._tix_call("subwidgets", "-all")
        except Exception:  # noqa: BLE001
            return None
        try:
            return self.tk.splitlist(names)
        except Exception:  # noqa: BLE001
            return None

    def config_all(self, option, value):
        if option in (None, ""):
            return None
        opt_name = _normalize_option_name(option)
        names = self._subwidget_names() or ()
        for name in names:
            self.tk.call(name, "configure", opt_name, value)
        self._tix_call("configure", opt_name, value)
        return None

    def image_create(self, imgtype, cnf=None, master=None, **kw):
        target = self if master is None else master
        return target.tk.call("image", "create", imgtype, *_options_tuple(cnf, **kw))

    def image_delete(self, imgname):
        try:
            return self.tk.call("image", "delete", imgname)
        except Exception:  # noqa: BLE001
            return None


class TixSubWidget(TixWidget):
    """Lightweight wrapper around an existing Tix subwidget path."""

    def __init__(self, master, name):
        if not isinstance(master, _tkinter.Misc):
            raise TypeError("TixSubWidget master must be a tkinter widget")
        self.master = master
        self.tk = master.tk
        self._tk_app = master._tk_app
        self.children = {}
        self.subwidget_list = {}
        try:
            path = master.call(master._w, "subwidget", name)
        except Exception:  # noqa: BLE001
            path = f"{master._w}.{name}"
        self._w = str(path)

    def destroy(self):
        try:
            return self.tk.call("destroy", self._w)
        except Exception:  # noqa: BLE001
            return None


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


def OptionName(widget):
    text = str(widget)
    if text.startswith("*"):
        return text
    return f"*{text}"


def FileTypeList(dict_or_seq):
    if isinstance(dict_or_seq, dict):
        return [(key, value) for key, value in dict_or_seq.items()]
    if isinstance(dict_or_seq, (list, tuple)):
        return list(dict_or_seq)
    return [dict_or_seq]


class InputOnly:
    """Marker mixin used by legacy Tix wrappers."""


class Control(TixWidget):
    _widget_command = "tixControl"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def decrement(self):
        return self._tix_call("decr")

    def increment(self):
        return self._tix_call("incr")

    def invoke(self):
        return self._tix_call("invoke")

    def update(self):
        return self._tix_call("update")


class ComboBox(TixWidget):
    _widget_command = "tixComboBox"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def add_history(self, str):
        return self._tix_call("addhistory", str)

    def append_history(self, str):
        return self._tix_call("appendhistory", str)

    def insert(self, index, str):
        return self._tix_call("insert", index, str)

    def pick(self, index):
        return self._tix_call("pick", index)


class DirList(TixWidget):
    _widget_command = "tixDirList"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def chdir(self, dir):
        return self._tix_call("chdir", dir)


class DirTree(TixWidget):
    _widget_command = "tixDirTree"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def chdir(self, dir):
        return self._tix_call("chdir", dir)


class DirSelectBox(TixWidget):
    _widget_command = "tixDirSelectBox"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)


class ExFileSelectBox(TixWidget):
    _widget_command = "tixExFileSelectBox"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def filter(self):
        return self._tix_call("filter")

    def invoke(self):
        return self._tix_call("invoke")


class DirSelectDialog(TixWidget):
    _widget_command = "tixDirSelectDialog"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def popup(self):
        return self._tix_call("popup")

    def popdown(self):
        return self._tix_call("popdown")


class ExFileSelectDialog(TixWidget):
    _widget_command = "tixExFileSelectDialog"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def popup(self):
        return self._tix_call("popup")

    def popdown(self):
        return self._tix_call("popdown")


class FileSelectBox(TixWidget):
    _widget_command = "tixFileSelectBox"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def apply_filter(self):
        return self._tix_call("filter")

    def invoke(self):
        return self._tix_call("invoke")


class FileSelectDialog(TixWidget):
    _widget_command = "tixFileSelectDialog"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def popup(self):
        return self._tix_call("popup")

    def popdown(self):
        return self._tix_call("popdown")


class FileEntry(TixWidget):
    _widget_command = "tixFileEntry"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def invoke(self):
        return self._tix_call("invoke")

    def file_dialog(self):
        return self._tix_call("filedialog")


class HList(TixWidget):
    _widget_command = "tixHList"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def add(self, entry, cnf=None, **kw):
        return self._tix_call("add", entry, *_options_tuple(cnf, **kw))

    def add_child(self, parent=None, cnf=None, **kw):
        target = "" if parent is None else parent
        return self._tix_call("addchild", target, *_options_tuple(cnf, **kw))

    def anchor_set(self, entry):
        return self._tix_call("anchor", "set", entry)

    def anchor_clear(self):
        return self._tix_call("anchor", "clear")

    def column_width(self, col=0, width=None, chars=None):
        if chars is None:
            return self._tix_call("column", "width", col, width)
        return self._tix_call("column", "width", col, "-char", chars)

    def delete_all(self):
        return self._tix_call("delete", "all")

    def delete_entry(self, entry):
        return self._tix_call("delete", "entry", entry)

    def delete_offsprings(self, entry):
        return self._tix_call("delete", "offsprings", entry)

    def delete_siblings(self, entry):
        return self._tix_call("delete", "siblings", entry)

    def dragsite_set(self, index):
        return self._tix_call("dragsite", "set", index)

    def dragsite_clear(self):
        return self._tix_call("dragsite", "clear")

    def dropsite_set(self, index):
        return self._tix_call("dropsite", "set", index)

    def dropsite_clear(self):
        return self._tix_call("dropsite", "clear")

    def header_create(self, col, cnf=None, **kw):
        return self._tix_call("header", "create", col, *_options_tuple(cnf, **kw))

    def header_configure(self, col, cnf=None, **kw):
        if cnf is None and not kw:
            return self._tix_call("header", "configure", col)
        return self._tix_call("header", "configure", col, *_options_tuple(cnf, **kw))

    def header_cget(self, col, opt):
        return self._tix_call("header", "cget", col, opt)

    def header_exists(self, col):
        result = self._tix_call("header", "exist", col)
        return bool(self.getboolean(result))

    header_exist = header_exists

    def header_delete(self, col):
        return self._tix_call("header", "delete", col)

    def header_size(self, col):
        return self._tix_call("header", "size", col)

    def hide_entry(self, entry):
        return self._tix_call("hide", "entry", entry)

    def indicator_create(self, entry, cnf=None, **kw):
        return self._tix_call("indicator", "create", entry, *_options_tuple(cnf, **kw))

    def indicator_configure(self, entry, cnf=None, **kw):
        if cnf is None and not kw:
            return self._tix_call("indicator", "configure", entry)
        return self._tix_call(
            "indicator", "configure", entry, *_options_tuple(cnf, **kw)
        )

    def indicator_cget(self, entry, opt):
        return self._tix_call("indicator", "cget", entry, opt)

    def indicator_exists(self, entry):
        return bool(self.getboolean(self._tix_call("indicator", "exists", entry)))

    def indicator_delete(self, entry):
        return self._tix_call("indicator", "delete", entry)

    def indicator_size(self, entry):
        return self._tix_call("indicator", "size", entry)

    def info_anchor(self):
        return self._tix_call("info", "anchor")

    def info_bbox(self, entry):
        result = self._tix_call("info", "bbox", entry)
        return _split_ints(self, result) or None

    def info_children(self, entry=None):
        result = self._tix_call("info", "children", entry)
        return self.tk.splitlist(result)

    def info_data(self, entry):
        return self._tix_call("info", "data", entry)

    def info_dragsite(self):
        return self._tix_call("info", "dragsite")

    def info_dropsite(self):
        return self._tix_call("info", "dropsite")

    def info_exists(self, entry):
        return bool(self.getboolean(self._tix_call("info", "exists", entry)))

    def info_hidden(self, entry):
        return bool(self.getboolean(self._tix_call("info", "hidden", entry)))

    def info_next(self, entry):
        return self._tix_call("info", "next", entry)

    def info_parent(self, entry):
        return self._tix_call("info", "parent", entry)

    def info_prev(self, entry):
        return self._tix_call("info", "prev", entry)

    def info_selection(self):
        result = self._tix_call("info", "selection")
        return self.tk.splitlist(result)

    def item_cget(self, entry, col, opt):
        return self._tix_call("item", "cget", entry, col, opt)

    def item_configure(self, entry, col, cnf=None, **kw):
        if cnf is None and not kw:
            return self._tix_call("item", "configure", entry, col)
        return self._tix_call(
            "item", "configure", entry, col, *_options_tuple(cnf, **kw)
        )

    def item_create(self, entry, col, cnf=None, **kw):
        return self._tix_call("item", "create", entry, col, *_options_tuple(cnf, **kw))

    def item_exists(self, entry, col):
        return bool(self.getboolean(self._tix_call("item", "exists", entry, col)))

    def item_delete(self, entry, col):
        return self._tix_call("item", "delete", entry, col)

    def entrycget(self, entry, opt):
        return self._tix_call("entrycget", entry, opt)

    def entryconfigure(self, entry, cnf=None, **kw):
        if cnf is None and not kw:
            return self._tix_call("entryconfigure", entry)
        return self._tix_call("entryconfigure", entry, *_options_tuple(cnf, **kw))

    def nearest(self, y):
        return self._tix_call("nearest", y)

    def see(self, entry):
        return self._tix_call("see", entry)

    def selection_clear(self, cnf=None, **kw):
        return self._tix_call("selection", "clear", *_options_tuple(cnf, **kw))

    def selection_includes(self, entry):
        return bool(self.getboolean(self._tix_call("selection", "includes", entry)))

    def selection_set(self, first, last=None):
        if last is None:
            return self._tix_call("selection", "set", first)
        return self._tix_call("selection", "set", first, last)

    def show_entry(self, entry):
        return self._tix_call("show", "entry", entry)


class CheckList(TixWidget):
    _widget_command = "tixCheckList"

    def autosetmode(self):
        return self._tix_call("autosetmode")

    def close(self, entrypath=None):
        if entrypath is None:
            return self._tix_call("close")
        return self._tix_call("close", entrypath)

    def getmode(self, entrypath):
        return self._tix_call("getmode", entrypath)

    def open(self, entrypath=None):
        if entrypath is None:
            return self._tix_call("open")
        return self._tix_call("open", entrypath)

    def getselection(self, mode="on"):
        return self.tk.splitlist(self._tix_call("getselection", mode))

    def getstatus(self, entrypath):
        return self._tix_call("getstatus", entrypath)

    def setstatus(self, entrypath, mode="on"):
        return self._tix_call("setstatus", entrypath, mode)


class TList(TixWidget):
    _widget_command = "tixTList"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def active_set(self, index):
        return self._tix_call("active", "set", index)

    def active_clear(self):
        return self._tix_call("active", "clear")

    def anchor_set(self, index):
        return self._tix_call("anchor", "set", index)

    def anchor_clear(self):
        return self._tix_call("anchor", "clear")

    def delete(self, from_, to=None):
        if to is None:
            return self._tix_call("delete", from_)
        return self._tix_call("delete", from_, to)

    def dragsite_set(self, index):
        return self._tix_call("dragsite", "set", index)

    def dragsite_clear(self):
        return self._tix_call("dragsite", "clear")

    def dropsite_set(self, index):
        return self._tix_call("dropsite", "set", index)

    def dropsite_clear(self):
        return self._tix_call("dropsite", "clear")

    def insert(self, index, cnf=None, **kw):
        return self._tix_call("insert", index, *_options_tuple(cnf, **kw))

    def info_active(self):
        return self._tix_call("info", "active")

    def info_anchor(self):
        return self._tix_call("info", "anchor")

    def info_down(self, index):
        return self._tix_call("info", "down", index)

    def info_left(self, index):
        return self._tix_call("info", "left", index)

    def info_right(self, index):
        return self._tix_call("info", "right", index)

    def info_selection(self):
        return self.tk.splitlist(self._tix_call("info", "selection"))

    def info_size(self):
        return self._tix_call("info", "size")

    def info_up(self, index):
        return self._tix_call("info", "up", index)

    def nearest(self, x, y):
        return self._tix_call("nearest", x, y)

    def see(self, index):
        return self._tix_call("see", index)

    def selection_clear(self, cnf=None, **kw):
        return self._tix_call("selection", "clear", *_options_tuple(cnf, **kw))

    def selection_includes(self, index):
        return bool(self.getboolean(self._tix_call("selection", "includes", index)))

    def selection_set(self, first, last=None):
        if last is None:
            return self._tix_call("selection", "set", first)
        return self._tix_call("selection", "set", first, last)


class Tree(TixWidget):
    _widget_command = "tixTree"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def autosetmode(self):
        return self._tix_call("autosetmode")

    def close(self, entrypath):
        return self._tix_call("close", entrypath)

    def getmode(self, entrypath):
        return self._tix_call("getmode", entrypath)

    def open(self, entrypath):
        return self._tix_call("open", entrypath)

    def setmode(self, entrypath, mode="none"):
        return self._tix_call("setmode", entrypath, mode)


class NoteBook(TixWidget):
    _widget_command = "tixNoteBook"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def add(self, name, cnf=None, **kw):
        self._tix_call("add", name, *_options_tuple(cnf, **kw))
        widget = self.subwidget(name)
        self.subwidget_list[str(name)] = widget
        return widget

    def delete(self, name):
        result = self._tix_call("delete", name)
        self.subwidget_list.pop(str(name), None)
        return result

    def page(self, name):
        return self.subwidget(name)

    def pages(self):
        names = self.tk.splitlist(self._tix_call("pages"))
        return [self.subwidget(name) for name in names]

    def raise_page(self, name):
        return self._tix_call("raise", name)

    def raised(self):
        return self._tix_call("raised")


class NoteBookFrame(TixWidget):
    _widget_command = "tixNoteBookFrame"


class PanedWindow(TixWidget):
    _widget_command = "tixPanedWindow"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def add(self, name, cnf=None, **kw):
        self._tix_call("add", name, *_options_tuple(cnf, **kw))
        widget = self.subwidget(name)
        self.subwidget_list[str(name)] = widget
        return widget

    def delete(self, name):
        result = self._tix_call("delete", name)
        self.subwidget_list.pop(str(name), None)
        return result

    def forget(self, name):
        return self._tix_call("forget", name)

    def panecget(self, entry, opt):
        return self._tix_call("panecget", entry, opt)

    def paneconfigure(self, entry, cnf=None, **kw):
        if cnf is None and not kw:
            return self._tix_call("paneconfigure", entry)
        return self._tix_call("paneconfigure", entry, *_options_tuple(cnf, **kw))

    def panes(self):
        names = self.tk.splitlist(self._tix_call("panes"))
        return [self.subwidget(name) for name in names]


class ListNoteBook(TixWidget):
    _widget_command = "tixListNoteBook"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def add(self, name, cnf=None, **kw):
        self._tix_call("add", name, *_options_tuple(cnf, **kw))
        widget = self.subwidget(name)
        self.subwidget_list[str(name)] = widget
        return widget

    def page(self, name):
        return self.subwidget(name)

    def pages(self):
        names = self.tk.splitlist(self._tix_call("pages"))
        return [self.subwidget(name) for name in names]

    def raise_page(self, name):
        return self._tix_call("raise", name)


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

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def invoke(self, name):
        return self._tix_call("invoke", name)


class ButtonBox(TixWidget):
    _widget_command = "tixButtonBox"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def add(self, name, cnf=None, **kw):
        return self._tix_call("add", name, *_options_tuple(cnf, **kw))

    def invoke(self, name):
        return self._tix_call("invoke", name)


class Balloon(TixWidget):
    _widget_command = "tixBalloon"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def bind_widget(self, widget, cnf=None, **kw):
        return self._tix_call("bind", _widget_path(widget), *_options_tuple(cnf, **kw))

    def unbind_widget(self, widget):
        return self._tix_call("unbind", _widget_path(widget))


class ResizeHandle(TixWidget):
    _widget_command = "tixResizeHandle"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def attach_widget(self, widget):
        return self._tix_call("attachwidget", _widget_path(widget))

    def detach_widget(self, widget):
        return self._tix_call("detachwidget", _widget_path(widget))

    def hide(self, widget):
        return self._tix_call("hide", _widget_path(widget))

    def show(self, widget):
        return self._tix_call("show", _widget_path(widget))


class LabelEntry(TixWidget):
    _widget_command = "tixLabelEntry"


class LabelFrame(TixWidget):
    _widget_command = "tixLabelFrame"


class MainWindow(TixWidget):
    _widget_command = "tixMainWindow"


class Select(TixWidget):
    _widget_command = "tixSelect"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def add(self, name, cnf=None, **kw):
        return self._tix_call("add", name, *_options_tuple(cnf, **kw))

    def invoke(self, name):
        return self._tix_call("invoke", name)


class Shell(TixWidget):
    _widget_command = "tixShell"


class DialogShell(TixWidget):
    _widget_command = "tixDialogShell"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def popdown(self):
        return self._tix_call("popdown")

    def popup(self):
        return self._tix_call("popup")

    def center(self):
        return self._tix_call("center")


class PopupMenu(TixWidget):
    _widget_command = "tixPopupMenu"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def bind_widget(self, widget):
        return self._tix_call("bind", _widget_path(widget))

    def unbind_widget(self, widget):
        return self._tix_call("unbind", _widget_path(widget))

    def post_widget(self, widget, x, y):
        return self._tix_call("post", _widget_path(widget), x, y)


class OptionMenu(TixWidget):
    _widget_command = "tixOptionMenu"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def add_command(self, name, cnf=None, **kw):
        return self._tix_call("add", "command", name, *_options_tuple(cnf, **kw))

    def add_separator(self, name, cnf=None, **kw):
        return self._tix_call("add", "separator", name, *_options_tuple(cnf, **kw))

    def delete(self, name):
        return self._tix_call("delete", name)

    def disable(self, name):
        return self._tix_call("disable", name)

    def enable(self, name):
        return self._tix_call("enable", name)


class CObjView(TixWidget):
    _widget_command = "tixCObjView"


class Grid(TixWidget):
    _widget_command = "tixGrid"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf=cnf, **kw)

    def anchor_clear(self):
        return self._tix_call("anchor", "clear")

    def anchor_get(self):
        return self._tix_call("anchor", "get")

    def anchor_set(self, x, y):
        return self._tix_call("anchor", "set", x, y)

    def delete_row(self, from_, to=None):
        if to is None:
            return self._tix_call("delete", "row", from_)
        return self._tix_call("delete", "row", from_, to)

    def delete_column(self, from_, to=None):
        if to is None:
            return self._tix_call("delete", "column", from_)
        return self._tix_call("delete", "column", from_, to)

    def edit_apply(self):
        return self._tix_call("edit", "apply")

    def edit_set(self, x, y):
        return self._tix_call("edit", "set", x, y)

    def entrycget(self, x, y, option):
        return self._tix_call("entrycget", x, y, option)

    def entryconfigure(self, x, y, cnf=None, **kw):
        if cnf is None and not kw:
            return self._tix_call("entryconfigure", x, y)
        return self._tix_call("entryconfigure", x, y, *_options_tuple(cnf, **kw))

    def info_exists(self, x, y):
        return bool(self.getboolean(self._tix_call("info", "exists", x, y)))

    def info_bbox(self, x, y):
        result = self._tix_call("info", "bbox", x, y)
        return _split_ints(self, result) or None

    def move_column(self, from_, to, offset):
        return self._tix_call("move", "column", from_, to, offset)

    def move_row(self, from_, to, offset):
        return self._tix_call("move", "row", from_, to, offset)

    def nearest(self, x, y):
        return _split_ints(self, self._tix_call("nearest", x, y))

    def set(self, x, y, itemtype=None, **kw):
        if itemtype is None:
            if kw:
                return self._tix_call("set", x, y, *_options_tuple(None, **kw))
            return self._tix_call("set", x, y)
        return self._tix_call("set", x, y, itemtype, *_options_tuple(None, **kw))

    def size_column(self, index, **kw):
        if not kw:
            return self._tix_call("size", "column", index)
        return self._tix_call("size", "column", index, *_options_tuple(None, **kw))

    def size_row(self, index, **kw):
        if not kw:
            return self._tix_call("size", "row", index)
        return self._tix_call("size", "row", index, *_options_tuple(None, **kw))

    def unset(self, x, y):
        return self._tix_call("unset", x, y)


class Form(TixWidget):
    _widget_command = "tixForm"

    def config(self, cnf=None, **kw):
        return self.tk.call("tixForm", self._w, *_options_tuple(cnf, **kw))

    form = config

    def __setitem__(self, key, value):
        self.form({key: value})

    def check(self):
        return self.tk.call("tixForm", "check", self._w)

    def forget(self):
        return self.tk.call("tixForm", "forget", self._w)

    def grid(self, xsize=0, ysize=0):
        if not xsize and not ysize:
            return _split_ints(self, self.tk.call("tixForm", "grid", self._w))
        return self.tk.call("tixForm", "grid", self._w, xsize, ysize)

    def info(self, option=None):
        if not option:
            return self.tk.call("tixForm", "info", self._w)
        return self.tk.call("tixForm", "info", self._w, _normalize_option_name(option))

    def slaves(self):
        return self.tk.splitlist(self.tk.call("tixForm", "slaves", self._w))


def _init_dummy_subwidget(widget, master, name):
    TixSubWidget.__init__(widget, master, name)


class _dummyButton(_tkinter.Button, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyCheckbutton(_tkinter.Checkbutton, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyEntry(_tkinter.Entry, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyFrame(_tkinter.Frame, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyLabel(_tkinter.Label, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyListbox(_tkinter.Listbox, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyMenu(_tkinter.Menu, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyMenubutton(_tkinter.Menubutton, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyScrollbar(_tkinter.Scrollbar, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyText(_tkinter.Text, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyScrolledListBox(ScrolledListBox, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["listbox"] = _dummyListbox(self, "listbox")
        self.subwidget_list["vsb"] = _dummyScrollbar(self, "vsb")
        self.subwidget_list["hsb"] = _dummyScrollbar(self, "hsb")


class _dummyHList(HList, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyScrolledHList(ScrolledHList, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["hlist"] = _dummyHList(self, "hlist")
        self.subwidget_list["vsb"] = _dummyScrollbar(self, "vsb")
        self.subwidget_list["hsb"] = _dummyScrollbar(self, "hsb")


class _dummyTList(TList, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


class _dummyComboBox(ComboBox, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["label"] = _dummyLabel(self, "label")
        self.subwidget_list["entry"] = _dummyEntry(self, "entry")
        self.subwidget_list["arrow"] = _dummyButton(self, "arrow")
        self.subwidget_list["slistbox"] = _dummyScrolledListBox(self, "slistbox")
        try:
            self.subwidget_list["tick"] = _dummyButton(self, "tick")
            self.subwidget_list["cross"] = _dummyButton(self, "cross")
        except Exception:  # noqa: BLE001
            pass


class _dummyDirList(DirList, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["hlist"] = _dummyHList(self, "hlist")
        self.subwidget_list["vsb"] = _dummyScrollbar(self, "vsb")
        self.subwidget_list["hsb"] = _dummyScrollbar(self, "hsb")


class _dummyDirSelectBox(DirSelectBox, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["dirlist"] = _dummyDirList(self, "dirlist")
        self.subwidget_list["dircbx"] = _dummyFileComboBox(self, "dircbx")


class _dummyExFileSelectBox(ExFileSelectBox, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["cancel"] = _dummyButton(self, "cancel")
        self.subwidget_list["ok"] = _dummyButton(self, "ok")
        self.subwidget_list["hidden"] = _dummyCheckbutton(self, "hidden")
        self.subwidget_list["types"] = _dummyComboBox(self, "types")
        self.subwidget_list["dir"] = _dummyComboBox(self, "dir")
        self.subwidget_list["dirlist"] = _dummyScrolledListBox(self, "dirlist")
        self.subwidget_list["file"] = _dummyComboBox(self, "file")
        self.subwidget_list["filelist"] = _dummyScrolledListBox(self, "filelist")


class _dummyFileSelectBox(FileSelectBox, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["dirlist"] = _dummyScrolledListBox(self, "dirlist")
        self.subwidget_list["filelist"] = _dummyScrolledListBox(self, "filelist")
        self.subwidget_list["filter"] = _dummyComboBox(self, "filter")
        self.subwidget_list["selection"] = _dummyComboBox(self, "selection")


class _dummyFileComboBox(ComboBox, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["dircbx"] = _dummyComboBox(self, "dircbx")


class _dummyStdButtonBox(StdButtonBox, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)
        self.subwidget_list["ok"] = _dummyButton(self, "ok")
        self.subwidget_list["apply"] = _dummyButton(self, "apply")
        self.subwidget_list["cancel"] = _dummyButton(self, "cancel")
        self.subwidget_list["help"] = _dummyButton(self, "help")


class _dummyNoteBookFrame(NoteBookFrame, TixSubWidget):
    def __init__(self, master, name, destroy_physically=0):
        _init_dummy_subwidget(self, master, name)


class _dummyPanedWindow(PanedWindow, TixSubWidget):
    def __init__(self, master, name, destroy_physically=1):
        _init_dummy_subwidget(self, master, name)


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
