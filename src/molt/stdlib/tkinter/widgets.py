"""Widget, image, and variable classes for intrinsic-backed tkinter."""

import sys
import warnings

from .constants import RAISED
from . import (
    CallWrapper,
    Event,
    Misc,
    TclError,
    Wm,
    XView,
    YView,
    _event_from_subst_args,
    _flatten,
    _get_default_root,
    _normalize_bind_add,
    _normalize_bind_target,
    _normalize_option_name,
    _normalize_tk_options,
    _normalize_trace_mode,
    _next_variable_name,
    _TK_TEXT_TAG_BIND_REGISTER,
    _TK_TEXT_TAG_BIND_UNREGISTER,
    _TK_TRACE_ADD,
    _TK_TRACE_CLEAR,
    _TK_TRACE_INFO,
    _TK_TRACE_REMOVE,
    _TK_WIDGET_BIND_REGISTER,
    _TK_WIDGET_BIND_UNREGISTER,
)


class Widget(Misc):
    """Widget shell used by tkinter/ttk wrappers."""

    def __init__(self, master, widget_command, cnf=None, **kw):
        parent = _get_default_root() if master is None else master
        if not isinstance(parent, Misc):
            raise TypeError("widget master must be a tkinter widget or root")
        root = parent.tk
        self.master = parent
        self.tk = root
        self._tk_app = root._tk_app
        self._name, self._w = root._next_widget_path(parent._w, widget_command)
        self.children = {}
        if hasattr(parent, "children"):
            parent.children[self._name] = self
        argv = [widget_command, self._w]
        argv.extend(_normalize_tk_options(cnf, owner=self, **kw))
        self.tk.call(*argv)

    def _root(self):
        """Return the root Toplevel (Tk) widget for this widget."""
        root = self.tk
        return root

    def destroy(self):
        try:
            super().destroy()
        finally:
            if hasattr(self.master, "children"):
                self.master.children.pop(getattr(self, "_name", self._w), None)

    def __str__(self):
        return self._w


class BaseWidget(Widget):
    """Compatibility alias for CPython's internal BaseWidget."""

    def destroy(self):
        return super().destroy()


class _CoreWidget(Widget):
    _widget_command = "widget"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, self._widget_command, cnf, **kw)


class Button(_CoreWidget):
    _widget_command = "button"

    def flash(self):
        return self._call_widget("flash")

    def invoke(self):
        return self._call_widget("invoke")


class Label(_CoreWidget):
    _widget_command = "label"


class Entry(_CoreWidget):
    _widget_command = "entry"

    xview = XView.xview
    xview_moveto = XView.xview_moveto
    xview_scroll = XView.xview_scroll

    def bbox(self, index):
        result = self._call_widget("bbox", index)
        if not result:
            return None
        return tuple(self.getint(part) for part in self.splitlist(result))

    def delete(self, first, last=None):
        if last is None:
            return self._call_widget("delete", first)
        return self._call_widget("delete", first, last)

    def get(self):
        return self._call_widget("get")

    def icursor(self, index):
        return self._call_widget("icursor", index)

    def index(self, index):
        return self.getint(self._call_widget("index", index))

    def insert(self, index, string):
        return self._call_widget("insert", index, string)

    def scan_mark(self, x):
        return self._call_widget("scan", "mark", x)

    def scan_dragto(self, x):
        return self._call_widget("scan", "dragto", x)

    def selection_adjust(self, index):
        return self._call_widget("selection", "adjust", index)

    def selection_clear(self):
        return self._call_widget("selection", "clear")

    def selection_from(self, index):
        return self._call_widget("selection", "from", index)

    def selection_present(self):
        return bool(self.getint(self._call_widget("selection", "present")))

    def selection_range(self, start, end):
        return self._call_widget("selection", "range", start, end)

    def selection_to(self, index):
        return self._call_widget("selection", "to", index)

    def validate(self):
        return bool(self.getboolean(self._call_widget("validate")))

    select_adjust = selection_adjust
    select_clear = selection_clear
    select_from = selection_from
    select_present = selection_present
    select_range = selection_range
    select_to = selection_to


class Frame(_CoreWidget):
    _widget_command = "frame"


class Canvas(_CoreWidget):
    _widget_command = "canvas"

    xview = XView.xview
    xview_moveto = XView.xview_moveto
    xview_scroll = XView.xview_scroll
    yview = YView.yview
    yview_moveto = YView.yview_moveto
    yview_scroll = YView.yview_scroll

    def addtag(self, *args):
        return self._call_widget("addtag", *args)

    def addtag_above(self, newtag, tag_or_id):
        return self._call_widget("addtag", "above", newtag, tag_or_id)

    def addtag_all(self, newtag):
        return self._call_widget("addtag", "all", newtag)

    def addtag_below(self, newtag, tag_or_id):
        return self._call_widget("addtag", "below", newtag, tag_or_id)

    def addtag_closest(self, newtag, x, y, halo=None, start=None):
        args = ["closest", newtag, x, y]
        if halo is not None:
            args.append(halo)
        if start is not None:
            args.append(start)
        return self._call_widget("addtag", *args)

    def addtag_enclosed(self, newtag, x1, y1, x2, y2):
        return self._call_widget("addtag", "enclosed", newtag, x1, y1, x2, y2)

    def addtag_overlapping(self, newtag, x1, y1, x2, y2):
        return self._call_widget("addtag", "overlapping", newtag, x1, y1, x2, y2)

    def addtag_withtag(self, newtag, tag_or_id):
        return self._call_widget("addtag", "withtag", newtag, tag_or_id)

    def bbox(self, *args):
        result = self._call_widget("bbox", *args)
        if not result:
            return None
        return tuple(self.getint(part) for part in self.splitlist(result))

    def tag_unbind(self, tag_or_id, sequence, funcid=None):
        if funcid is None:
            return self._call_widget("bind", tag_or_id, sequence, "")
        command_name = str(funcid)
        _TK_WIDGET_BIND_UNREGISTER(
            self._tk_app,
            self._w,
            tag_or_id,
            sequence,
            command_name,
        )
        return None

    def tag_bind(self, tag_or_id, sequence=None, func=None, add=None):
        if func is None:
            if sequence is None:
                return self._call_widget("bind", tag_or_id)
            return self._call_widget("bind", tag_or_id, sequence)
        if sequence is None:
            raise TypeError(
                "tag_bind sequence must not be None when callback is provided"
            )
        add_prefix = _normalize_bind_add(add)
        if isinstance(func, str):
            script = f"{add_prefix}{func}" if add_prefix else func
            self._call_widget("bind", tag_or_id, sequence, script)
            return func
        if not callable(func):
            raise TypeError("tag_bind callback must be callable")

        widget = self

        def wrapped(*event_args):
            parsed_event = _event_from_subst_args(widget, event_args)
            if parsed_event is not None:
                return func(parsed_event)
            if event_args:
                return func(*event_args)
            event = Event()
            event.widget = widget
            return func(event)

        return _TK_WIDGET_BIND_REGISTER(
            self._tk_app,
            self._w,
            tag_or_id,
            sequence,
            wrapped,
            add_prefix,
        )

    def canvasx(self, screenx, gridspacing=None):
        if gridspacing is None:
            return self.getdouble(self._call_widget("canvasx", screenx))
        return self.getdouble(self._call_widget("canvasx", screenx, gridspacing))

    def canvasy(self, screeny, gridspacing=None):
        if gridspacing is None:
            return self.getdouble(self._call_widget("canvasy", screeny))
        return self.getdouble(self._call_widget("canvasy", screeny, gridspacing))

    def coords(self, *args):
        result = self._call_widget("coords", *args)
        if not result:
            return ()
        return tuple(self.getdouble(part) for part in self.splitlist(result))

    def _create(self, item_type, args, kw):
        flat_args = list(_flatten(args))
        flat_args.extend(_normalize_tk_options(None, owner=self, **kw))
        return self.getint(self._call_widget("create", item_type, *flat_args))

    def create_arc(self, *args, **kw):
        return self._create("arc", args, kw)

    def create_bitmap(self, *args, **kw):
        return self._create("bitmap", args, kw)

    def create_image(self, *args, **kw):
        return self._create("image", args, kw)

    def create_line(self, *args, **kw):
        return self._create("line", args, kw)

    def create_oval(self, *args, **kw):
        return self._create("oval", args, kw)

    def create_polygon(self, *args, **kw):
        return self._create("polygon", args, kw)

    def create_rectangle(self, *args, **kw):
        return self._create("rectangle", args, kw)

    def create_text(self, *args, **kw):
        return self._create("text", args, kw)

    def create_window(self, *args, **kw):
        return self._create("window", args, kw)

    def dchars(self, *args):
        return self._call_widget("dchars", *args)

    def delete(self, *args):
        return self._call_widget("delete", *args)

    def dtag(self, *args):
        return self._call_widget("dtag", *args)

    def find(self, *args):
        return tuple(
            self.getint(part)
            for part in self.splitlist(self._call_widget("find", *args))
        )

    def find_above(self, tag_or_id):
        return self.find("above", tag_or_id)

    def find_all(self):
        return self.find("all")

    def find_below(self, tag_or_id):
        return self.find("below", tag_or_id)

    def find_closest(self, x, y, halo=None, start=None):
        args = [x, y]
        if halo is not None:
            args.append(halo)
        if start is not None:
            args.append(start)
        return self.find("closest", *args)

    def find_enclosed(self, x1, y1, x2, y2):
        return self.find("enclosed", x1, y1, x2, y2)

    def find_overlapping(self, x1, y1, x2, y2):
        return self.find("overlapping", x1, y1, x2, y2)

    def find_withtag(self, tag_or_id):
        return self.find("withtag", tag_or_id)

    def focus(self, *args):
        return self._call_widget("focus", *args)

    def gettags(self, *args):
        return self.splitlist(self._call_widget("gettags", *args))

    def icursor(self, *args):
        return self._call_widget("icursor", *args)

    def index(self, *args):
        result = self._call_widget("index", *args)
        if isinstance(result, int):
            return result
        if isinstance(result, str) and result.lstrip("-").isdigit():
            return self.getint(result)
        return result

    def insert(self, *args):
        return self._call_widget("insert", *args)

    def itemcget(self, tag_or_id, option):
        return self._call_widget("itemcget", tag_or_id, _normalize_option_name(option))

    def itemconfigure(self, tag_or_id, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "itemconfigure() option query cannot be combined with updates"
                )
            return self._call_widget(
                "itemconfigure", tag_or_id, _normalize_option_name(cnf)
            )
        if cnf is None and not kw:
            return self._call_widget("itemconfigure", tag_or_id)
        return self._call_widget(
            "itemconfigure", tag_or_id, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def tag_lower(self, *args):
        return self._call_widget("lower", *args)

    def move(self, *args):
        return self._call_widget("move", *args)

    def moveto(self, tag_or_id, x="", y=""):
        return self._call_widget("moveto", tag_or_id, x, y)

    def postscript(self, cnf=None, **kw):
        return self._call_widget(
            "postscript", *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def tag_raise(self, *args):
        return self._call_widget("raise", *args)

    def scale(self, *args):
        return self._call_widget("scale", *args)

    def scan_mark(self, x, y):
        return self._call_widget("scan", "mark", x, y)

    def scan_dragto(self, x, y, gain=10):
        return self._call_widget("scan", "dragto", x, y, gain)

    def select_adjust(self, tag_or_id, index):
        return self._call_widget("select", "adjust", tag_or_id, index)

    def select_clear(self):
        return self._call_widget("select", "clear")

    def select_from(self, tag_or_id, index):
        return self._call_widget("select", "from", tag_or_id, index)

    def select_item(self):
        return self._call_widget("select", "item")

    def select_to(self, tag_or_id, index):
        return self._call_widget("select", "to", tag_or_id, index)

    def type(self, tag_or_id):
        return self._call_widget("type", tag_or_id)

    itemconfig = itemconfigure
    lower = tag_lower
    lift = tkraise = tag_raise


class Text(_CoreWidget):
    _widget_command = "text"

    xview = XView.xview
    xview_moveto = XView.xview_moveto
    xview_scroll = XView.xview_scroll
    yview = YView.yview
    yview_moveto = YView.yview_moveto
    yview_scroll = YView.yview_scroll

    def bbox(self, index):
        result = self._call_widget("bbox", index)
        if not result:
            return None
        return tuple(self.getint(part) for part in self.splitlist(result))

    def compare(self, index1, op, index2):
        return bool(self.getboolean(self._call_widget("compare", index1, op, index2)))

    def count(self, index1, index2, *args):
        switches = [_normalize_option_name(arg) for arg in args]
        result = self._call_widget("count", *switches, index1, index2)
        if not result:
            return None
        return tuple(self.getint(part) for part in self.splitlist(result))

    def debug(self, boolean=None):
        if boolean is None:
            return bool(self.getboolean(self._call_widget("debug")))
        return self._call_widget("debug", int(bool(boolean)))

    def delete(self, index1, index2=None):
        if index2 is None:
            return self._call_widget("delete", index1)
        return self._call_widget("delete", index1, index2)

    def dlineinfo(self, index):
        result = self._call_widget("dlineinfo", index)
        if not result:
            return None
        return tuple(self.getint(part) for part in self.splitlist(result))

    def dump(self, index1, index2=None, command=None, **kw):
        args = []
        callback_name = None
        for key, value in kw.items():
            option = _normalize_option_name(str(key))
            if isinstance(value, bool):
                if value:
                    args.append(option)
                continue
            args.append(option)
            if value is not None:
                args.append(value)
        if command is not None:
            if isinstance(command, str):
                callback_name = command
            elif callable(command):
                callback_name = self._register_command("text_dump", command)
            else:
                raise TypeError("dump() command must be callable or command name")
            args.extend(("-command", callback_name))
        args.append(index1)
        if index2 is not None:
            args.append(index2)
        try:
            result = self._call_widget("dump", *args)
        finally:
            if command is not None and callable(command):
                self._release_command(callback_name)
        if command is not None:
            return None
        return self.splitlist(result)

    def edit(self, *args):
        return self._call_widget("edit", *args)

    def edit_modified(self, arg=None):
        if arg is None:
            return bool(self.getboolean(self._call_widget("edit", "modified")))
        return self._call_widget("edit", "modified", arg)

    def edit_redo(self):
        return self._call_widget("edit", "redo")

    def edit_reset(self):
        return self._call_widget("edit", "reset")

    def edit_separator(self):
        return self._call_widget("edit", "separator")

    def edit_undo(self):
        return self._call_widget("edit", "undo")

    def get(self, index1, index2=None):
        if index2 is None:
            return self._call_widget("get", index1)
        return self._call_widget("get", index1, index2)

    def image_cget(self, index, option):
        return self._call_widget("image", "cget", index, _normalize_option_name(option))

    def image_configure(self, index, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "image_configure() option query cannot be combined with updates"
                )
            return self._call_widget(
                "image", "configure", index, _normalize_option_name(cnf)
            )
        if cnf is None and not kw:
            return self._call_widget("image", "configure", index)
        return self._call_widget(
            "image", "configure", index, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def image_create(self, index, cnf=None, **kw):
        return self._call_widget(
            "image", "create", index, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def image_names(self):
        return self.splitlist(self._call_widget("image", "names"))

    def index(self, index):
        return self._call_widget("index", index)

    def insert(self, index, chars, *args):
        return self._call_widget("insert", index, chars, *args)

    def mark_gravity(self, mark_name, direction=None):
        if direction is None:
            return self._call_widget("mark", "gravity", mark_name)
        return self._call_widget("mark", "gravity", mark_name, direction)

    def mark_names(self):
        return self.splitlist(self._call_widget("mark", "names"))

    def mark_set(self, mark_name, index):
        return self._call_widget("mark", "set", mark_name, index)

    def mark_unset(self, *mark_names):
        return self._call_widget("mark", "unset", *mark_names)

    def mark_next(self, index):
        value = self._call_widget("mark", "next", index)
        return None if value == "" else value

    def mark_previous(self, index):
        value = self._call_widget("mark", "previous", index)
        return None if value == "" else value

    def peer_create(self, new_path_name, cnf=None, **kw):
        return self._call_widget(
            "peer",
            "create",
            new_path_name,
            *_normalize_tk_options(cnf, owner=self, **kw),
        )

    def peer_names(self):
        return self.splitlist(self._call_widget("peer", "names"))

    def replace(self, index1, index2, chars, *args):
        return self._call_widget("replace", index1, index2, chars, *args)

    def scan_mark(self, x, y):
        return self._call_widget("scan", "mark", x, y)

    def scan_dragto(self, x, y):
        return self._call_widget("scan", "dragto", x, y)

    def search(
        self,
        pattern,
        index,
        stopindex=None,
        forwards=None,
        backwards=None,
        exact=None,
        regexp=None,
        nocase=None,
        count=None,
        elide=None,
    ):
        args = []
        if forwards:
            args.append("-forwards")
        if backwards:
            args.append("-backwards")
        if exact:
            args.append("-exact")
        if regexp:
            args.append("-regexp")
        if nocase:
            args.append("-nocase")
        if elide:
            args.append("-elide")
        if count is not None:
            count_name = count._name if hasattr(count, "_name") else count
            args.extend(("-count", count_name))
        args.extend((pattern, index))
        if stopindex is not None:
            args.append(stopindex)
        return self._call_widget("search", *args)

    def see(self, index):
        return self._call_widget("see", index)

    def tag_add(self, tag_name, index1, *args):
        return self._call_widget("tag", "add", tag_name, index1, *args)

    def tag_unbind(self, tag_name, sequence, funcid=None):
        if funcid is None:
            return self._call_widget("tag", "bind", tag_name, sequence, "")
        command_name = str(funcid)
        _TK_TEXT_TAG_BIND_UNREGISTER(
            self._tk_app,
            self._w,
            tag_name,
            sequence,
            command_name,
        )
        return None

    def tag_bind(self, tag_name, sequence=None, func=None, add=None):
        return self._tag_bind(tag_name, sequence, func, add)

    def _tag_bind(self, tag_name, sequence=None, func=None, add=None):
        if func is None:
            if sequence is None:
                return self._call_widget("tag", "bind", tag_name)
            return self._call_widget("tag", "bind", tag_name, sequence)
        if sequence is None:
            raise TypeError(
                "tag_bind sequence must not be None when callback is provided"
            )
        add_prefix = _normalize_bind_add(add)
        if isinstance(func, str):
            script = f"{add_prefix}{func}" if add_prefix else func
            self._call_widget("tag", "bind", tag_name, sequence, script)
            return func
        if not callable(func):
            raise TypeError("tag_bind callback must be callable")

        widget = self

        def wrapped(*event_args):
            parsed_event = _event_from_subst_args(widget, event_args)
            if parsed_event is not None:
                return func(parsed_event)
            if event_args:
                return func(*event_args)
            event = Event()
            event.widget = widget
            return func(event)

        return _TK_TEXT_TAG_BIND_REGISTER(
            self._tk_app,
            self._w,
            tag_name,
            sequence,
            wrapped,
            add_prefix,
        )

    def tag_cget(self, tag_name, option):
        return self._call_widget(
            "tag", "cget", tag_name, _normalize_option_name(option)
        )

    def tag_configure(self, tag_name, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "tag_configure() option query cannot be combined with updates"
                )
            return self._call_widget(
                "tag", "configure", tag_name, _normalize_option_name(cnf)
            )
        if cnf is None and not kw:
            return self._call_widget("tag", "configure", tag_name)
        return self._call_widget(
            "tag", "configure", tag_name, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def tag_delete(self, *tag_names):
        return self._call_widget("tag", "delete", *tag_names)

    def tag_lower(self, tag_name, below_this=None):
        if below_this is None:
            return self._call_widget("tag", "lower", tag_name)
        return self._call_widget("tag", "lower", tag_name, below_this)

    def tag_names(self, index=None):
        if index is None:
            return self.splitlist(self._call_widget("tag", "names"))
        return self.splitlist(self._call_widget("tag", "names", index))

    def tag_nextrange(self, tag_name, index1, index2=None):
        if index2 is None:
            return self.splitlist(
                self._call_widget("tag", "nextrange", tag_name, index1)
            )
        return self.splitlist(
            self._call_widget("tag", "nextrange", tag_name, index1, index2)
        )

    def tag_prevrange(self, tag_name, index1, index2=None):
        if index2 is None:
            return self.splitlist(
                self._call_widget("tag", "prevrange", tag_name, index1)
            )
        return self.splitlist(
            self._call_widget("tag", "prevrange", tag_name, index1, index2)
        )

    def tag_raise(self, tag_name, above_this=None):
        if above_this is None:
            return self._call_widget("tag", "raise", tag_name)
        return self._call_widget("tag", "raise", tag_name, above_this)

    def tag_ranges(self, tag_name):
        return self.splitlist(self._call_widget("tag", "ranges", tag_name))

    def tag_remove(self, tag_name, index1, index2=None):
        if index2 is None:
            return self._call_widget("tag", "remove", tag_name, index1)
        return self._call_widget("tag", "remove", tag_name, index1, index2)

    def window_cget(self, index, option):
        return self._call_widget(
            "window", "cget", index, _normalize_option_name(option)
        )

    def window_configure(self, index, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "window_configure() option query cannot be combined with updates"
                )
            return self._call_widget(
                "window", "configure", index, _normalize_option_name(cnf)
            )
        if cnf is None and not kw:
            return self._call_widget("window", "configure", index)
        return self._call_widget(
            "window", "configure", index, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def window_create(self, index, cnf=None, **kw):
        return self._call_widget(
            "window", "create", index, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def window_names(self):
        return self.splitlist(self._call_widget("window", "names"))

    def yview_pickplace(self, *what):
        return self._call_widget("yview", "pickplace", *what)

    tag_config = tag_configure
    window_config = window_configure


class Toplevel(_CoreWidget, Wm):
    """Toplevel widget with window manager controls (title, geometry, protocol, etc.)."""

    _widget_command = "toplevel"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, cnf, **kw)
        self._protocol_commands = {}

    def _wm_call(self, command, *args):
        return self.call("wm", command, self._w, *args)

    def destroy(self):
        for command_name in list(getattr(self, "_protocol_commands", {}).values()):
            self._release_command(command_name)
        self._protocol_commands = {}
        super().destroy()


class Listbox(_CoreWidget):
    _widget_command = "listbox"

    xview = XView.xview
    xview_moveto = XView.xview_moveto
    xview_scroll = XView.xview_scroll
    yview = YView.yview
    yview_moveto = YView.yview_moveto
    yview_scroll = YView.yview_scroll

    def activate(self, index):
        return self._call_widget("activate", index)

    def bbox(self, index):
        result = self._call_widget("bbox", index)
        if not result:
            return None
        return tuple(self.getint(part) for part in self.splitlist(result))

    def curselection(self):
        return tuple(
            self.getint(part)
            for part in self.splitlist(self._call_widget("curselection"))
        )

    def delete(self, first, last=None):
        if last is None:
            return self._call_widget("delete", first)
        return self._call_widget("delete", first, last)

    def get(self, first, last=None):
        if last is None:
            return self._call_widget("get", first)
        return self.splitlist(self._call_widget("get", first, last))

    def index(self, index):
        return self.getint(self._call_widget("index", index))

    def insert(self, index, *elements):
        return self._call_widget("insert", index, *elements)

    def nearest(self, y):
        return self.getint(self._call_widget("nearest", y))

    def scan_mark(self, x, y):
        return self._call_widget("scan", "mark", x, y)

    def scan_dragto(self, x, y):
        return self._call_widget("scan", "dragto", x, y)

    def see(self, index):
        return self._call_widget("see", index)

    def selection_anchor(self, index):
        return self._call_widget("selection", "anchor", index)

    def selection_clear(self, first, last=None):
        if last is None:
            return self._call_widget("selection", "clear", first)
        return self._call_widget("selection", "clear", first, last)

    def selection_includes(self, index):
        return bool(self.getint(self._call_widget("selection", "includes", index)))

    def selection_set(self, first, last=None):
        if last is None:
            return self._call_widget("selection", "set", first)
        return self._call_widget("selection", "set", first, last)

    def size(self):
        return self.getint(self._call_widget("size"))

    def itemcget(self, index, option):
        return self._call_widget("itemcget", index, _normalize_option_name(option))

    def itemconfigure(self, index, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "itemconfigure() option query cannot be combined with updates"
                )
            return self._call_widget(
                "itemconfigure", index, _normalize_option_name(cnf)
            )
        if cnf is None and not kw:
            return self._call_widget("itemconfigure", index)
        return self._call_widget(
            "itemconfigure", index, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    select_anchor = selection_anchor
    select_clear = selection_clear
    select_includes = selection_includes
    select_set = selection_set
    itemconfig = itemconfigure


class Menu(_CoreWidget):
    _widget_command = "menu"

    def tk_popup(self, x, y, entry=""):
        if entry == "":
            return self.call("tk_popup", self._w, x, y)
        return self.call("tk_popup", self._w, x, y, entry)

    def activate(self, index):
        return self._call_widget("activate", index)

    def add(self, item_type, cnf=None, **kw):
        return self._call_widget(
            "add", item_type, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def add_cascade(self, cnf=None, **kw):
        return self.add("cascade", cnf, **kw)

    def add_checkbutton(self, cnf=None, **kw):
        return self.add("checkbutton", cnf, **kw)

    def add_command(self, cnf=None, **kw):
        return self._call_widget(
            "add", "command", *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def add_radiobutton(self, cnf=None, **kw):
        return self.add("radiobutton", cnf, **kw)

    def add_separator(self, cnf=None, **kw):
        return self.add("separator", cnf, **kw)

    def insert(self, index, item_type, cnf=None, **kw):
        return self._call_widget(
            "insert", index, item_type, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def insert_cascade(self, index, cnf=None, **kw):
        return self.insert(index, "cascade", cnf, **kw)

    def insert_checkbutton(self, index, cnf=None, **kw):
        return self.insert(index, "checkbutton", cnf, **kw)

    def insert_command(self, index, cnf=None, **kw):
        return self.insert(index, "command", cnf, **kw)

    def insert_radiobutton(self, index, cnf=None, **kw):
        return self.insert(index, "radiobutton", cnf, **kw)

    def insert_separator(self, index, cnf=None, **kw):
        return self.insert(index, "separator", cnf, **kw)

    def delete(self, index1, index2=None):
        if index2 is None:
            return self._call_widget("delete", index1)
        return self._call_widget("delete", index1, index2)

    def entrycget(self, index, option):
        return self._call_widget("entrycget", index, _normalize_option_name(option))

    def entryconfigure(self, index, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "entryconfigure() option query cannot be combined with updates"
                )
            return self._call_widget(
                "entryconfigure", index, _normalize_option_name(cnf)
            )
        if cnf is None and not kw:
            return self._call_widget("entryconfigure", index)
        return self._call_widget(
            "entryconfigure", index, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    def index(self, index):
        return self._call_widget("index", index)

    def invoke(self, index):
        return self._call_widget("invoke", index)

    def post(self, x, y):
        return self._call_widget("post", x, y)

    def type(self, index):
        return self._call_widget("type", index)

    def unpost(self):
        return self._call_widget("unpost")

    def xposition(self, index):
        return self.getint(self._call_widget("xposition", index))

    def yposition(self, index):
        return self.getint(self._call_widget("yposition", index))

    entryconfig = entryconfigure
    entryindex = index


class Scrollbar(_CoreWidget):
    _widget_command = "scrollbar"

    def activate(self, index=None):
        if index is None:
            return self._call_widget("activate")
        return self._call_widget("activate", index)

    def delta(self, deltax, deltay):
        return self.getdouble(self._call_widget("delta", deltax, deltay))

    def fraction(self, x, y):
        return self.getdouble(self._call_widget("fraction", x, y))

    def identify(self, x, y):
        return self._call_widget("identify", x, y)

    def get(self):
        result = self._call_widget("get")
        return tuple(self.getdouble(part) for part in self.splitlist(result))

    def set(self, first, last):
        return self._call_widget("set", first, last)


class Menubutton(_CoreWidget):
    _widget_command = "menubutton"


class Checkbutton(_CoreWidget):
    _widget_command = "checkbutton"

    def deselect(self):
        return self._call_widget("deselect")

    def flash(self):
        return self._call_widget("flash")

    def invoke(self):
        return self._call_widget("invoke")

    def select(self):
        return self._call_widget("select")

    def toggle(self):
        return self._call_widget("toggle")


class Radiobutton(_CoreWidget):
    _widget_command = "radiobutton"

    def deselect(self):
        return self._call_widget("deselect")

    def flash(self):
        return self._call_widget("flash")

    def invoke(self):
        return self._call_widget("invoke")

    def select(self):
        return self._call_widget("select")


class Spinbox(_CoreWidget):
    _widget_command = "spinbox"

    xview = XView.xview
    xview_moveto = XView.xview_moveto
    xview_scroll = XView.xview_scroll

    def bbox(self, index):
        result = self._call_widget("bbox", index)
        if not result:
            return None
        return tuple(self.getint(part) for part in self.splitlist(result))

    def delete(self, first, last=None):
        if last is None:
            return self._call_widget("delete", first)
        return self._call_widget("delete", first, last)

    def get(self):
        return self._call_widget("get")

    def icursor(self, index):
        return self._call_widget("icursor", index)

    def identify(self, x, y):
        return self._call_widget("identify", x, y)

    def index(self, index):
        return self.getint(self._call_widget("index", index))

    def insert(self, index, s):
        return self._call_widget("insert", index, s)

    def invoke(self, element):
        return self._call_widget("invoke", element)

    def scan(self, *args):
        return self._call_widget("scan", *args)

    def scan_mark(self, x):
        return self._call_widget("scan", "mark", x)

    def scan_dragto(self, x):
        return self._call_widget("scan", "dragto", x)

    def selection(self, *args):
        return self._call_widget("selection", *args)

    def selection_adjust(self, index):
        return self._call_widget("selection", "adjust", index)

    def selection_clear(self):
        return self._call_widget("selection", "clear")

    def selection_element(self, element=None):
        if element is None:
            return self._call_widget("selection", "element")
        return self._call_widget("selection", "element", element)

    def selection_from(self, index):
        return self._call_widget("selection", "from", index)

    def selection_present(self):
        return bool(self.getint(self._call_widget("selection", "present")))

    def selection_range(self, start, end):
        return self._call_widget("selection", "range", start, end)

    def selection_to(self, index):
        return self._call_widget("selection", "to", index)

    def validate(self):
        return bool(self.getboolean(self._call_widget("validate")))


class Scale(_CoreWidget):
    _widget_command = "scale"

    def get(self):
        return self.getdouble(self._call_widget("get"))

    def set(self, value):
        return self._call_widget("set", value)

    def coords(self, value=None):
        if value is None:
            result = self._call_widget("coords")
        else:
            result = self._call_widget("coords", value)
        if not result:
            return ()
        return tuple(self.getdouble(part) for part in self.splitlist(result))

    def identify(self, x, y):
        return self._call_widget("identify", x, y)


class PanedWindow(_CoreWidget):
    _widget_command = "panedwindow"

    def add(self, child, **kw):
        return self._call_widget(
            "add",
            _normalize_bind_target(child),
            *_normalize_tk_options(None, owner=self, **kw),
        )

    def remove(self, child):
        return self._call_widget("forget", _normalize_bind_target(child))

    forget = remove

    def identify(self, x, y):
        return self._call_widget("identify", x, y)

    def proxy(self, *args):
        result = self._call_widget("proxy", *args)
        if not result:
            return ()
        return tuple(self.getint(part) for part in self.splitlist(result))

    def proxy_coord(self):
        return self.proxy("coord")

    def proxy_forget(self):
        return self.proxy("forget")

    def proxy_place(self, x, y):
        return self.proxy("place", x, y)

    def sash(self, *args):
        result = self._call_widget("sash", *args)
        if not result:
            return ()
        return tuple(self.getint(part) for part in self.splitlist(result))

    def sash_coord(self, index):
        return self.sash("coord", index)

    def sash_mark(self, index):
        return self.sash("mark", index)

    def sash_place(self, index, x, y):
        return self.sash("place", index, x, y)

    def panecget(self, child, option):
        return self._call_widget(
            "panecget", _normalize_bind_target(child), _normalize_option_name(option)
        )

    def paneconfigure(self, tag_or_id, cnf=None, **kw):
        target = _normalize_bind_target(tag_or_id)
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "paneconfigure() option query cannot be combined with updates"
                )
            return self._call_widget(
                "paneconfigure", target, _normalize_option_name(cnf)
            )
        if cnf is None and not kw:
            return self._call_widget("paneconfigure", target)
        return self._call_widget(
            "paneconfigure", target, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    paneconfig = paneconfigure

    def panes(self):
        return self.splitlist(self._call_widget("panes"))


class LabelFrame(_CoreWidget):
    _widget_command = "labelframe"


class Message(_CoreWidget):
    _widget_command = "message"


class _setit:
    """Internal class. It wraps the command in the widget OptionMenu."""

    def __init__(self, var, value, callback=None):
        self.__value = value
        self.__var = var
        self.__callback = callback

    def __call__(self, *args):
        self.__var.set(self.__value)
        if self.__callback is not None:
            self.__callback(self.__value, *args)


class OptionMenu(Menubutton):
    def __init__(self, master, variable, value, *values, **kwargs):
        callback = kwargs.pop("command", None)
        if kwargs:
            raise TclError(f'unknown option "-{next(iter(kwargs))}"')
        super().__init__(
            master,
            textvariable=variable,
            indicatoron=1,
            relief=RAISED,  # noqa: F405
            anchor="c",
            highlightthickness=2,
            borderwidth=2,
        )
        self.widgetName = "tk_optionMenu"
        menu = Menu(self, tearoff=0)
        self.__menu = menu
        self.menuname = menu._w
        self._optionmenu_commands = []

        for candidate in (value, *values):
            command = self._register_command(
                "optionmenu", _setit(variable, candidate, callback)
            )
            self._optionmenu_commands.append(command)
            menu.add_command(label=candidate, command=command)
        self["menu"] = menu

    def __getitem__(self, name):
        if name == "menu":
            return self.__menu
        return super().__getitem__(name)

    def destroy(self):
        for command in getattr(self, "_optionmenu_commands", ()):
            self._release_command(command)
        self._optionmenu_commands = []
        self.__menu = None
        super().destroy()


class Image:
    _last_id = 0

    def __init__(self, imgtype, name=None, cnf=None, master=None, **kw):
        if master is None:
            master = _get_default_root("create image")
        self.tk = master.tk if isinstance(master, Misc) else master
        if not name:
            Image._last_id += 1
            name = f"pyimage{Image._last_id}"
        options = _normalize_tk_options(cnf, owner=self, **kw)
        self.tk.call("image", "create", imgtype, name, *options)
        self.name = name

    def __str__(self):
        return self.name

    def __del__(self):
        if getattr(self, "name", None):
            tk = getattr(self, "tk", None)
            if tk is not None:
                call = getattr(tk, "call", None)
                if callable(call):
                    call("image", "delete", self.name)

    def __setitem__(self, key, value):
        self.tk.call(self.name, "configure", _normalize_option_name(key), value)

    def __getitem__(self, key):
        return self.tk.call(self.name, "cget", _normalize_option_name(key))

    def configure(self, cnf=None, **kw):
        return self.tk.call(
            self.name, "configure", *_normalize_tk_options(cnf, owner=self, **kw)
        )

    config = configure

    def cget(self, key):
        return self.tk.call(self.name, "cget", _normalize_option_name(key))

    def height(self):
        return self.tk.getint(self.tk.call("image", "height", self.name))

    def type(self):
        return self.tk.call("image", "type", self.name)

    def width(self):
        return self.tk.getint(self.tk.call("image", "width", self.name))


class PhotoImage(Image):
    def __init__(self, name=None, cnf=None, master=None, **kw):
        super().__init__("photo", name, cnf, master, **kw)

    def blank(self):
        return self.tk.call(self.name, "blank")

    def cget(self, option):
        return self.tk.call(self.name, "cget", _normalize_option_name(option))

    def data(self, format=None, from_coords=None):
        """Return the image data as a string."""
        args = [self.name, "data"]
        if format is not None:
            args.extend(("-format", format))
        if from_coords is not None:
            args.extend(("-from", *from_coords))
        return self.tk.call(*args)

    def copy(self):
        dest_image = PhotoImage(master=self.tk)
        self.tk.call(dest_image, "copy", self.name)
        return dest_image

    def zoom(self, x, y=""):
        if y == "":
            y = x
        dest_image = PhotoImage(master=self.tk)
        self.tk.call(dest_image, "copy", self.name, "-zoom", x, y)
        return dest_image

    def subsample(self, x, y=""):
        if y == "":
            y = x
        dest_image = PhotoImage(master=self.tk)
        self.tk.call(dest_image, "copy", self.name, "-subsample", x, y)
        return dest_image

    def get(self, x, y):
        return self.tk.call(self.name, "get", x, y)

    def put(self, data, to=None):
        if to is None:
            return self.tk.call(self.name, "put", data)
        return self.tk.call(self.name, "put", data, "-to", *to)

    def write(self, filename, format=None, from_coords=None):
        args = [filename]
        if format is not None:
            args.extend(("-format", format))
        if from_coords is not None:
            args.extend(("-from", *from_coords))
        return self.tk.call(self.name, "write", *args)

    def transparency_get(self, x, y):
        return bool(
            self.tk.getboolean(self.tk.call(self.name, "transparency", "get", x, y))
        )

    def transparency_set(self, x, y, boolean):
        return self.tk.call(
            self.name,
            "transparency",
            "set",
            x,
            y,
            int(bool(boolean)),
        )

    if sys.version_info >= (3, 13):

        def copy_replace(
            self,
            sourceImage,
            from_coords=None,
            to=None,
            shrink=False,
            zoom=None,
            subsample=None,
            compositingrule=None,
        ):
            """Copy a region from a source image into this image."""
            args = [str(sourceImage)]
            if from_coords is not None:
                args.extend(["-from"] + [str(c) for c in from_coords])
            if to is not None:
                args.extend(["-to"] + [str(c) for c in to])
            if shrink:
                args.append("-shrink")
            if zoom is not None:
                if isinstance(zoom, (list, tuple)):
                    args.extend(["-zoom"] + [str(z) for z in zoom])
                else:
                    args.extend(["-zoom", str(zoom)])
            if subsample is not None:
                if isinstance(subsample, (list, tuple)):
                    args.extend(["-subsample"] + [str(s) for s in subsample])
                else:
                    args.extend(["-subsample", str(subsample)])
            if compositingrule is not None:
                args.extend(["-compositingrule", str(compositingrule)])
            self.tk.call(self.name, "copy", *args)


class BitmapImage(Image):
    def __init__(self, name=None, cnf=None, master=None, **kw):
        super().__init__("bitmap", name, cnf, master, **kw)


def image_names():
    tk = _get_default_root("use image_names()").tk
    return tk.splitlist(tk.call("image", "names"))


def image_types():
    tk = _get_default_root("use image_types()").tk
    return tk.splitlist(tk.call("image", "types"))


class Variable:
    _default = ""
    _tk = None
    _tclCommands = None

    def __init__(self, master=None, value=None, name=None):
        parent = _get_default_root() if master is None else master
        if not isinstance(parent, Misc):
            raise TypeError("variable master must be a tkinter widget or root")
        self._root = parent.tk
        self._tk = parent.tk
        if name is None:
            self._name = _next_variable_name()
        else:
            self._name = str(name)
        if value is not None:
            self.set(value)
        elif name is None:
            self.set(self._default)

    @property
    def name(self):
        return self._name

    def __del__(self):
        tk = getattr(self, "_tk", None)
        name = getattr(self, "_name", None)
        if tk is None or name is None:
            return
        tk_app = getattr(tk, "_tk_app", None)
        if tk_app is not None:
            try:
                _TK_TRACE_CLEAR(tk_app, name)
            except RuntimeError as exc:
                if "intrinsic unavailable: molt_tk_trace_clear" not in str(exc):
                    raise
        getboolean = getattr(tk, "getboolean", None)
        call = getattr(tk, "call", None)
        if callable(getboolean) and callable(call):
            try:
                if getboolean(call("info", "exists", name)):
                    tk.globalunsetvar(name)
            except RuntimeError as exc:
                if "intrinsic unavailable: molt_capabilities_has" not in str(exc):
                    raise
        if self._tclCommands is not None:
            deletecommand = getattr(tk, "deletecommand", None)
            if callable(deletecommand):
                for command_name in self._tclCommands:
                    deletecommand(command_name)
            self._tclCommands = None

    def __str__(self):
        return self._name

    def __eq__(self, other):
        if not isinstance(other, Variable):
            return NotImplemented
        return (
            self._name == other._name
            and self.__class__.__name__ == other.__class__.__name__
            and self._tk == other._tk
        )

    def set(self, value):
        return self._tk.setvar(self._name, value)

    initialize = set

    def get(self):
        return self._tk.getvar(self._name)

    def _register(self, callback):
        wrapped = CallWrapper(callback, None, self._root).__call__
        command_name = repr(id(wrapped))
        func = getattr(callback, "__func__", None)
        if func is not None:
            callback = func
        cb_name = getattr(callback, "__name__", None)
        if cb_name is not None:
            command_name = command_name + cb_name
        self._tk.createcommand(command_name, wrapped)
        if self._tclCommands is None:
            self._tclCommands = []
        self._tclCommands.append(command_name)
        return command_name

    def trace_add(self, mode, callback):
        if not callable(callback):
            raise TypeError("trace callback must be callable")

        mode_name = _normalize_trace_mode(mode)

        def wrapped(*args):
            if args:
                return callback(*args)
            return callback(self._name, "", mode_name)

        return _TK_TRACE_ADD(self._tk._tk_app, self._name, mode_name, wrapped)

    def trace_remove(self, mode, cbname):
        mode_name = _normalize_trace_mode(mode)
        command_name = str(cbname)
        _TK_TRACE_REMOVE(self._tk._tk_app, self._name, mode_name, command_name)
        if self._tclCommands is not None and command_name in self._tclCommands:
            self._tclCommands.remove(command_name)
        return None

    def trace_info(self):
        rows = []
        for mode_name, callback_name in _TK_TRACE_INFO(self._tk._tk_app, self._name):
            rows.append((self._tk.splitlist(mode_name), callback_name))
        return rows

    def trace(self, mode, callback):
        return self.trace_add(mode, callback)

    def trace_variable(self, mode, callback):
        if sys.version_info >= (3, 14):
            warnings.warn(
                "trace_variable() is deprecated, use trace_add() instead",
                DeprecationWarning,
                stacklevel=2,
            )
        return self.trace_add(mode, callback)

    def trace_vdelete(self, mode, cbname):
        if sys.version_info >= (3, 14):
            warnings.warn(
                "trace_vdelete() is deprecated, use trace_remove() instead",
                DeprecationWarning,
                stacklevel=2,
            )
        return self.trace_remove(mode, cbname)

    def trace_vinfo(self):
        if sys.version_info >= (3, 14):
            warnings.warn(
                "trace_vinfo() is deprecated, use trace_info() instead",
                DeprecationWarning,
                stacklevel=2,
            )
        return self.trace_info()


class StringVar(Variable):
    _default = ""

    def get(self):
        return str(super().get())


class IntVar(Variable):
    _default = 0

    def get(self):
        return self._tk.getint(super().get())


class DoubleVar(Variable):
    _default = 0.0

    def get(self):
        return self._tk.getdouble(super().get())


class BooleanVar(Variable):
    _default = False

    def get(self):
        return self._tk.getboolean(super().get())

    def set(self, value):
        return super().set(bool(value))
