"""Phase-0 intrinsic-backed `tkinter.ttk` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic
from ._support import require_gui_capability as _require_gui_capability

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has", globals())
tkinter = _tkinter
_SUBST_FORMAT = (
    "%#",
    "%b",
    "%f",
    "%h",
    "%k",
    "%s",
    "%t",
    "%w",
    "%x",
    "%y",
    "%A",
    "%E",
    "%K",
    "%N",
    "%W",
    "%T",
    "%X",
    "%Y",
    "%D",
)
_SUBST_FORMAT_STR = " ".join(_SUBST_FORMAT)


def _normalize_option_name(name):
    return name if name.startswith("-") else f"-{name}"


def _normalize_options(cnf=None, **kw):
    if cnf is not None and not isinstance(cnf, dict):
        raise TypeError("ttk config must be a dict or None")

    merged = {}
    if cnf:
        merged.update(cnf)
    if kw:
        merged.update(kw)

    normalized = []
    for key, value in merged.items():
        if value is None:
            continue
        normalized.append(_normalize_option_name(str(key)))
        normalized.append(value)
    return normalized


def _normalize_statespec(statespec):
    if statespec is None:
        return []
    if isinstance(statespec, str):
        return [statespec]
    if isinstance(statespec, (list, tuple)):
        return list(statespec)
    raise TypeError("ttk state spec must be str, list, or tuple")


def _normalize_statespec_arg(statespec):
    normalized = _normalize_statespec(statespec)
    if not normalized:
        return ""
    return " ".join(str(item) for item in normalized)


def _flatten_items(items):
    if len(items) == 1 and isinstance(items[0], (tuple, list)):
        return tuple(items[0])
    return tuple(items)


def _call_with_options(
    tk,
    prefix,
    *,
    option=None,
    cnf=None,
    option_conflict_message="option cannot be combined with update options",
    **kw,
):
    if option is not None and (cnf or kw):
        raise TypeError(option_conflict_message)
    if option is not None:
        return tk.call(*prefix, _normalize_option_name(option))
    opts = _normalize_options(cnf, **kw)
    if opts:
        return tk.call(*prefix, *opts)
    return tk.call(*prefix)


def _event_int(tk, value):
    try:
        return tk.getint(value)
    except Exception:  # noqa: BLE001
        return value


def _event_from_subst_args(widget, event_args):
    args = list(event_args)
    if any(isinstance(item, tuple) and len(item) == 1 for item in args):
        args = [
            item[0] if isinstance(item, tuple) and len(item) == 1 else item
            for item in args
        ]
    if len(args) != len(_SUBST_FORMAT):
        return None

    (
        nsign,
        b,
        f,
        h,
        k,
        s,
        t,
        w,
        x,
        y,
        a,
        e_send,
        keysym,
        keysym_num,
        widget_path,
        ev_type,
        x_root,
        y_root,
        delta,
    ) = args

    event = _tkinter.Event()
    event.serial = _event_int(widget.tk, nsign)
    event.num = _event_int(widget.tk, b)
    try:
        event.focus = widget.tk.getboolean(f)
    except Exception:  # noqa: BLE001
        pass
    event.height = _event_int(widget.tk, h)
    event.keycode = _event_int(widget.tk, k)
    event.state = _event_int(widget.tk, s)
    event.time = _event_int(widget.tk, t)
    event.width = _event_int(widget.tk, w)
    event.x = _event_int(widget.tk, x)
    event.y = _event_int(widget.tk, y)
    event.char = a
    try:
        event.send_event = widget.tk.getboolean(e_send)
    except Exception:  # noqa: BLE001
        pass
    event.keysym = keysym
    event.keysym_num = _event_int(widget.tk, keysym_num)
    event.type = ev_type
    if isinstance(widget_path, str) and widget_path:
        event.widget = widget if widget_path == widget._w else widget_path
    else:
        event.widget = widget
    event.x_root = _event_int(widget.tk, x_root)
    event.y_root = _event_int(widget.tk, y_root)
    try:
        event.delta = widget.tk.getint(delta)
    except Exception:  # noqa: BLE001
        event.delta = 0 if delta in ("", None) else delta
    return event


def _split_pairs_to_dict(tk, value):
    if isinstance(value, dict):
        return value
    parts = tk.splitlist(value)
    if not parts:
        return {}
    if len(parts) % 2 != 0:
        return {}
    out = {}
    for index in range(0, len(parts), 2):
        out[str(parts[index])] = parts[index + 1]
    return out


def _convert_stringval(value):
    text = str(value)
    for converter in (int, float):
        try:
            return converter(text)
        except (TypeError, ValueError):
            pass
    return text


def _tclobj_to_py(value):
    if isinstance(value, tuple):
        if len(value) == 0:
            return ""
        return [_convert_stringval(item) for item in value]
    if isinstance(value, list):
        return [_convert_stringval(item) for item in value]
    if hasattr(value, "typename"):
        return _convert_stringval(value)
    return value


def tclobjs_to_py(adict):
    for opt, value in adict.items():
        adict[opt] = _tclobj_to_py(value)
    return adict


def setup_master(master=None):
    if master is None:
        return _tkinter._get_default_root()
    return master


class Widget(_tkinter.Widget):
    """Minimal ttk widget shell backed by tkinter.Widget/call routing."""

    _widget_command = "ttk::widget"

    def __init__(self, master=None, cnf=None, **kw):
        _require_gui_capability()
        super().__init__(master, self._widget_command, cnf, **kw)

    def state(self, statespec=None):
        _require_gui_capability()
        if statespec is None:
            return self.tk.call(self._w, "state")
        return self.tk.call(self._w, "state", *_normalize_statespec(statespec))

    def instate(self, statespec, callback=None, *args):
        _require_gui_capability()
        normalized = _normalize_statespec(statespec)
        if callback is None:
            return self.tk.call(self._w, "instate", *normalized)
        if not callable(callback):
            raise TypeError("instate callback must be callable")
        if self.tk.call(self._w, "instate", *normalized):
            return callback(*args)
        return None

    def identify(self, x, y):
        _require_gui_capability()
        return self.tk.call(self._w, "identify", x, y)


class Button(Widget):
    _widget_command = "ttk::button"

    def invoke(self):
        _require_gui_capability()
        return self.tk.call(self._w, "invoke")


class Checkbutton(Widget):
    _widget_command = "ttk::checkbutton"

    def invoke(self):
        _require_gui_capability()
        return self.tk.call(self._w, "invoke")


class Combobox(Widget):
    _widget_command = "ttk::combobox"

    def current(self, newindex=None):
        _require_gui_capability()
        if newindex is None:
            return self.tk.call(self._w, "current")
        return self.tk.call(self._w, "current", newindex)

    def set(self, value):
        _require_gui_capability()
        return self.tk.call(self._w, "set", value)


class Entry(Widget):
    _widget_command = "ttk::entry"

    def bbox(self, index):
        _require_gui_capability()
        return self.tk.call(self._w, "bbox", index)

    def identify(self, x, y):
        _require_gui_capability()
        return self.tk.call(self._w, "identify", x, y)

    def validate(self):
        _require_gui_capability()
        return self.tk.call(self._w, "validate")


class Frame(Widget):
    _widget_command = "ttk::frame"


class Label(Widget):
    _widget_command = "ttk::label"


class Labelframe(Widget):
    _widget_command = "ttk::labelframe"


LabelFrame = Labelframe


class Menubutton(Widget):
    _widget_command = "ttk::menubutton"


class Notebook(Widget):
    _widget_command = "ttk::notebook"

    def add(self, child, cnf=None, **kw):
        _require_gui_capability()
        opts = _normalize_options(cnf, **kw)
        return self.tk.call(self._w, "add", child, *opts)

    def forget(self, tab_id):
        _require_gui_capability()
        return self.tk.call(self._w, "forget", tab_id)

    def hide(self, tab_id):
        _require_gui_capability()
        return self.tk.call(self._w, "hide", tab_id)

    def identify(self, x, y):
        _require_gui_capability()
        return self.tk.call(self._w, "identify", x, y)

    def index(self, tab_id):
        _require_gui_capability()
        return self.tk.call(self._w, "index", tab_id)

    def insert(self, pos, child, cnf=None, **kw):
        _require_gui_capability()
        opts = _normalize_options(cnf, **kw)
        return self.tk.call(self._w, "insert", pos, child, *opts)

    def select(self, tab_id=None):
        _require_gui_capability()
        if tab_id is None:
            return self.tk.call(self._w, "select")
        return self.tk.call(self._w, "select", tab_id)

    def tab(self, tab_id, option=None, cnf=None, **kw):
        _require_gui_capability()
        return _call_with_options(
            self.tk,
            (self._w, "tab", tab_id),
            option=option,
            cnf=cnf,
            option_conflict_message=(
                "tab() option cannot be combined with update options"
            ),
            **kw,
        )

    def tabs(self):
        _require_gui_capability()
        return self.tk.call(self._w, "tabs")

    def enable_traversal(self):
        _require_gui_capability()
        return self.tk.call("ttk::notebook::enableTraversal", self._w)


class OptionMenu(Menubutton):
    """Minimal OptionMenu constructor shim over the Menubutton widget path."""

    def __init__(self, master, variable, default=None, *values, **kw):
        if default is not None:
            kw.setdefault("text", default)
        super().__init__(master, textvariable=variable, **kw)
        self._variable = variable
        self.variable = variable
        self.default = default
        self.values = tuple(values)

    def set_menu(self, default=None, *values):
        self.default = default
        self.values = tuple(values)
        set_value = getattr(self._variable, "set", None)
        if default is not None and callable(set_value):
            set_value(default)

    def destroy(self):
        try:
            del self._variable
        except AttributeError:
            pass
        try:
            del self.variable
        except AttributeError:
            pass
        super().destroy()


class Panedwindow(Widget):
    _widget_command = "ttk::panedwindow"

    def forget(self, pane):
        _require_gui_capability()
        return self.tk.call(self._w, "forget", pane)

    def insert(self, pos, child, cnf=None, **kw):
        _require_gui_capability()
        opts = _normalize_options(cnf, **kw)
        return self.tk.call(self._w, "insert", pos, child, *opts)

    def pane(self, pane, option=None, cnf=None, **kw):
        _require_gui_capability()
        return _call_with_options(
            self.tk,
            (self._w, "pane", pane),
            option=option,
            cnf=cnf,
            option_conflict_message=(
                "pane() option cannot be combined with update options"
            ),
            **kw,
        )

    def sashpos(self, index, newpos=None):
        _require_gui_capability()
        if newpos is None:
            return self.tk.call(self._w, "sashpos", index)
        return self.tk.call(self._w, "sashpos", index, newpos)


PanedWindow = Panedwindow


class Progressbar(Widget):
    _widget_command = "ttk::progressbar"

    def start(self, interval=None):
        _require_gui_capability()
        if interval is None:
            return self.tk.call(self._w, "start")
        return self.tk.call(self._w, "start", interval)

    def step(self, amount=None):
        _require_gui_capability()
        if amount is None:
            return self.tk.call(self._w, "step")
        return self.tk.call(self._w, "step", amount)

    def stop(self):
        _require_gui_capability()
        return self.tk.call(self._w, "stop")


class Radiobutton(Widget):
    _widget_command = "ttk::radiobutton"

    def invoke(self):
        _require_gui_capability()
        return self.tk.call(self._w, "invoke")


class Scale(Widget):
    _widget_command = "ttk::scale"

    def configure(self, cnf=None, **kw):
        _require_gui_capability()
        retval = super().configure(cnf, **kw)
        updated = {}
        if isinstance(cnf, dict):
            updated.update(cnf)
        if kw:
            updated.update(kw)
        if any(option in updated for option in ("from", "from_", "to")):
            self.event_generate("<<RangeChanged>>")
        return retval

    def get(self, x=None, y=None):
        _require_gui_capability()
        if x is None and y is None:
            return self.tk.call(self._w, "get")
        return self.tk.call(self._w, "get", x, y)


class LabeledScale(Frame):
    """Minimal ttk.LabeledScale compatibility wrapper."""

    def __init__(self, master=None, variable=None, from_=0, to=10, **kw):
        _require_gui_capability()
        self._label_top = kw.pop("compound", "top") == "top"
        super().__init__(master, **kw)
        self._variable = variable or _tkinter.IntVar(setup_master(master))
        self._variable.set(from_)

        self.label = Label(self)
        self.scale = Scale(self, variable=self._variable, from_=from_, to=to)

        scale_side = "bottom" if self._label_top else "top"
        label_side = "top" if self._label_top else "bottom"
        self.label.pack(side=label_side)
        self.scale.pack(side=scale_side, fill="x")

        trace_add = getattr(self._variable, "trace_add", None)
        self.__tracecb = None
        if callable(trace_add):
            self.__tracecb = trace_add("write", self._sync_label)
        self._sync_label()

    def destroy(self):
        trace_remove = getattr(self._variable, "trace_remove", None)
        if self.__tracecb is not None and callable(trace_remove):
            try:
                trace_remove("write", self.__tracecb)
            except Exception:  # noqa: BLE001
                pass
        try:
            del self._variable
        except AttributeError:
            pass
        super().destroy()
        self.label = None
        self.scale = None

    def _sync_label(self, *_args):
        if self.label is None:
            return None
        self.label.configure(text=self.value)
        return None

    @property
    def value(self):
        return self._variable.get()

    @value.setter
    def value(self, value):
        self._variable.set(value)


class Scrollbar(Widget):
    _widget_command = "ttk::scrollbar"


class Separator(Widget):
    _widget_command = "ttk::separator"


class Sizegrip(Widget):
    _widget_command = "ttk::sizegrip"


class Spinbox(Widget):
    _widget_command = "ttk::spinbox"

    def set(self, value):
        _require_gui_capability()
        return self.tk.call(self._w, "set", value)


class Treeview(Widget):
    _widget_command = "ttk::treeview"

    def bbox(self, item, column=None):
        _require_gui_capability()
        if column is None:
            return self._split_ints(self.tk.call(self._w, "bbox", item)) or ""
        return self._split_ints(self.tk.call(self._w, "bbox", item, column)) or ""

    def get_children(self, item=None):
        _require_gui_capability()
        return self.tk.splitlist(self.tk.call(self._w, "children", item or "") or ())

    def set_children(self, item, *newchildren):
        _require_gui_capability()
        return self.tk.call(self._w, "children", item, newchildren)

    def column(self, column, option=None, cnf=None, **kw):
        _require_gui_capability()
        return _call_with_options(
            self.tk,
            (self._w, "column", column),
            option=option,
            cnf=cnf,
            option_conflict_message=(
                "column() option cannot be combined with update options"
            ),
            **kw,
        )

    def delete(self, *items):
        _require_gui_capability()
        return self.tk.call(self._w, "delete", *items)

    def detach(self, *items):
        _require_gui_capability()
        return self.tk.call(self._w, "detach", *items)

    def exists(self, item):
        _require_gui_capability()
        return self.tk.getboolean(self.tk.call(self._w, "exists", item))

    def focus(self, item=None):
        _require_gui_capability()
        if item is None:
            return self.tk.call(self._w, "focus")
        return self.tk.call(self._w, "focus", item)

    def heading(self, column, option=None, cnf=None, **kw):
        _require_gui_capability()
        return _call_with_options(
            self.tk,
            (self._w, "heading", column),
            option=option,
            cnf=cnf,
            option_conflict_message=(
                "heading() option cannot be combined with update options"
            ),
            **kw,
        )

    def identify(self, component, x, y):
        _require_gui_capability()
        return self.tk.call(self._w, "identify", component, x, y)

    def identify_row(self, y):
        _require_gui_capability()
        return self.identify("row", 0, y)

    def identify_column(self, x):
        _require_gui_capability()
        return self.identify("column", x, 0)

    def identify_region(self, x, y):
        _require_gui_capability()
        return self.identify("region", x, y)

    def identify_element(self, x, y):
        _require_gui_capability()
        return self.identify("element", x, y)

    def index(self, item):
        _require_gui_capability()
        return self.tk.getint(self.tk.call(self._w, "index", item))

    def insert(self, parent, index, iid=None, cnf=None, **kw):
        _require_gui_capability()
        opts = _normalize_options(cnf, **kw)
        if iid is None:
            return self.tk.call(self._w, "insert", parent, index, *opts)
        return self.tk.call(self._w, "insert", parent, index, "-id", iid, *opts)

    def item(self, item, option=None, cnf=None, **kw):
        _require_gui_capability()
        return _call_with_options(
            self.tk,
            (self._w, "item", item),
            option=option,
            cnf=cnf,
            option_conflict_message=(
                "item() option cannot be combined with update options"
            ),
            **kw,
        )

    def move(self, item, parent, index):
        _require_gui_capability()
        return self.tk.call(self._w, "move", item, parent, index)

    reattach = move

    def next(self, item):
        _require_gui_capability()
        return self.tk.call(self._w, "next", item)

    def parent(self, item):
        _require_gui_capability()
        return self.tk.call(self._w, "parent", item)

    def prev(self, item):
        _require_gui_capability()
        return self.tk.call(self._w, "prev", item)

    def see(self, item):
        _require_gui_capability()
        return self.tk.call(self._w, "see", item)

    def selection(self):
        _require_gui_capability()
        return self.tk.splitlist(self.tk.call(self._w, "selection"))

    def _selection(self, op, items):
        _require_gui_capability()
        normalized_items = _flatten_items(items)
        return self.tk.call(self._w, "selection", op, *normalized_items)

    def selection_set(self, *items):
        return self._selection("set", items)

    def selection_add(self, *items):
        return self._selection("add", items)

    def selection_remove(self, *items):
        return self._selection("remove", items)

    def selection_toggle(self, *items):
        return self._selection("toggle", items)

    def set(self, item, column=None, value=None):
        _require_gui_capability()
        if column is None and value is None:
            return _split_pairs_to_dict(self.tk, self.tk.call(self._w, "set", item))
        if value is None:
            return self.tk.call(self._w, "set", item, column)
        return self.tk.call(self._w, "set", item, column, value)

    def tag_bind(self, tagname, sequence=None, callback=None):
        _require_gui_capability()
        if sequence is None:
            return self.tk.call(self._w, "tag", "bind", tagname)

        if callback is None:
            return self.tk.call(self._w, "tag", "bind", tagname, sequence)
        if isinstance(callback, str):
            self.tk.call(self._w, "tag", "bind", tagname, sequence, callback)
            return callback
        if not callable(callback):
            raise TypeError("tag_bind callback must be callable")

        def wrapped(*event_args):
            parsed_event = _event_from_subst_args(self, event_args)
            if parsed_event is not None:
                return callback(parsed_event)
            if event_args:
                return callback(*event_args)
            event = _tkinter.Event()
            event.widget = self
            return callback(event)

        command_name = self._register_command("ttk_tag_bind", wrapped)
        script = f'if {{"[{command_name} {_SUBST_FORMAT_STR}]" == "break"}} break\n'
        try:
            self.tk.call(self._w, "tag", "bind", tagname, sequence, script)
        except Exception:
            self._release_command(command_name)
            raise
        return command_name

    def tag_unbind(self, tagname, sequence, funcid=None):
        _require_gui_capability()
        if funcid is None:
            return self.tk.call(self._w, "tag", "bind", tagname, sequence, "")
        command_name = str(funcid)
        try:
            self.tk.call(self._w, "tag", "bind", tagname, sequence, "", command_name)
        except Exception:  # noqa: BLE001
            script = self.tk.call(self._w, "tag", "bind", tagname, sequence)
            replacement = ""
            if isinstance(script, str):
                prefix = f'if {{"[{command_name} '
                kept = [
                    line for line in script.split("\n") if not line.startswith(prefix)
                ]
                replacement = "\n".join(kept)
                if not replacement.strip():
                    replacement = ""
            self.tk.call(self._w, "tag", "bind", tagname, sequence, replacement)
        self._release_command(command_name)
        return None

    def tag_configure(self, tagname, option=None, cnf=None, **kw):
        _require_gui_capability()
        return _call_with_options(
            self.tk,
            (self._w, "tag", "configure", tagname),
            option=option,
            cnf=cnf,
            option_conflict_message=(
                "tag_configure() option cannot be combined with update options"
            ),
            **kw,
        )

    def tag_has(self, tagname, item=None):
        _require_gui_capability()
        if item is None:
            return self.tk.splitlist(self.tk.call(self._w, "tag", "has", tagname))
        return self.tk.getboolean(self.tk.call(self._w, "tag", "has", tagname, item))


class Style:
    """Thin ttk style wrapper over Tk calls."""

    def __init__(self, master=None):
        _require_gui_capability()
        parent = setup_master(master)
        if not isinstance(parent, _tkinter.Misc):
            raise TypeError("style master must be a tkinter widget or root")
        self.master = parent
        self.tk = parent.tk

    def configure(self, style, query_opt=None, cnf=None, **kw):
        _require_gui_capability()
        return _call_with_options(
            self.tk,
            ("ttk::style", "configure", style),
            option=query_opt,
            cnf=cnf,
            option_conflict_message=(
                "configure() query_opt cannot be combined with update options"
            ),
            **kw,
        )

    def map(self, style, query_opt=None, cnf=None, **kw):
        _require_gui_capability()
        return _call_with_options(
            self.tk,
            ("ttk::style", "map", style),
            option=query_opt,
            cnf=cnf,
            option_conflict_message=(
                "map() query_opt cannot be combined with update options"
            ),
            **kw,
        )

    def lookup(self, style, option, state=None, default=None):
        _require_gui_capability()
        state_arg = _normalize_statespec_arg(state)
        return self.tk.call(
            "ttk::style",
            "lookup",
            style,
            _normalize_option_name(option),
            state_arg,
            default,
        )

    def layout(self, style, layoutspec=None):
        _require_gui_capability()
        if layoutspec is None:
            return self.tk.call("ttk::style", "layout", style)
        return self.tk.call("ttk::style", "layout", style, layoutspec)

    def element_create(self, elementname, etype, *args):
        _require_gui_capability()
        return self.tk.call(
            "ttk::style",
            "element",
            "create",
            elementname,
            etype,
            *args,
        )

    def element_names(self):
        _require_gui_capability()
        return self.tk.call("ttk::style", "element", "names")

    def element_options(self, elementname):
        _require_gui_capability()
        return self.tk.call("ttk::style", "element", "options", elementname)

    def theme_create(self, themename, parent=None, settings=None):
        _require_gui_capability()
        argv = ["ttk::style", "theme", "create", themename]
        if parent is not None:
            argv.extend(["-parent", parent])
        if settings is not None:
            argv.extend(["-settings", settings])
        return self.tk.call(*argv)

    def theme_settings(self, themename, settings):
        _require_gui_capability()
        return self.tk.call("ttk::style", "theme", "settings", themename, settings)

    def theme_names(self):
        _require_gui_capability()
        return self.tk.call("ttk::style", "theme", "names")

    def theme_use(self, theme_name=None):
        _require_gui_capability()
        if theme_name is None:
            return self.tk.call("ttk::style", "theme", "use")
        return self.tk.call("ttk::style", "theme", "use", theme_name)


__all__ = [
    "Button",
    "Checkbutton",
    "Combobox",
    "Entry",
    "Frame",
    "Label",
    "LabelFrame",
    "LabeledScale",
    "Labelframe",
    "Menubutton",
    "Notebook",
    "OptionMenu",
    "PanedWindow",
    "Panedwindow",
    "Progressbar",
    "Radiobutton",
    "Scale",
    "Scrollbar",
    "Separator",
    "Sizegrip",
    "Spinbox",
    "Style",
    "Treeview",
    "Widget",
    "setup_master",
    "tclobjs_to_py",
]


def __getattr__(attr):
    raise AttributeError(
        f'module "{__name__}" has no attribute "{attr}"; '
        "only the Phase-0 ttk core surface is implemented."
    )
