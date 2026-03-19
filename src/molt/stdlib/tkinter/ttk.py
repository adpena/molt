"""Intrinsic-backed `tkinter.ttk` wrappers."""
import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic
from ._support import require_gui_capability as _require_gui_capability

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_MOLT_TK_EVENT_SUBST_PARSE = _require_intrinsic("molt_tk_event_subst_parse", globals())
_molt_tk_convert_stringval = _require_intrinsic("molt_tk_convert_stringval", globals())
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


def _require_tkinter_callable(attr):
    value = getattr(_tkinter, attr, None)
    if callable(value):
        return value
    raise RuntimeError(
        f"tkinter.ttk requires callable _tkinter.{attr} in the intrinsic runtime surface"
    )


_TK_TREEVIEW_TAG_BIND_REGISTER = _require_tkinter_callable("treeview_tag_bind_register")
_TK_TREEVIEW_TAG_BIND_UNREGISTER = _require_tkinter_callable(
    "treeview_tag_bind_unregister"
)


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


def _flatten(seq):
    out = []
    for item in seq:
        if isinstance(item, (tuple, list)):
            out.extend(_flatten(item))
        elif item is not None:
            out.append(item)
    return tuple(out)


def _format_optvalue(value, script=False):
    if script:
        stringify = getattr(_tkinter, "_stringify", None)
        if callable(stringify):
            return stringify(value)
        if isinstance(value, (list, tuple)):
            return "{" + " ".join(map(str, value)) + "}"
        return str(value)
    if isinstance(value, (list, tuple)):
        join = getattr(_tkinter, "_join", None)
        if callable(join):
            return join(value)
        return " ".join(map(str, value))
    return value


def _format_optdict(optdict, script=False, ignore=None):
    opts = []
    ignored = set(ignore or ())
    for opt, value in optdict.items():
        if opt in ignored:
            continue
        opts.append(f"-{opt}")
        if value is not None:
            opts.append(_format_optvalue(value, script))
    return _flatten(opts)


def _mapdict_values(items):
    opt_val = []
    for *state, val in items:
        if len(state) == 1:
            state = state[0] or ""
        else:
            state = " ".join(state)
        opt_val.append(state)
        if val is not None:
            opt_val.append(val)
    return opt_val


def _format_mapdict(mapdict, script=False):
    opts = []
    for opt, value in mapdict.items():
        opts.extend((f"-{opt}", _format_optvalue(_mapdict_values(value), script)))
    return _flatten(opts)


def _format_elemcreate(etype, script=False, *args, **kw):
    spec = None
    opts = ()
    if etype in ("image", "vsapi"):
        if etype == "image":
            image_name = args[0]
            image_spec = _format_optvalue(_mapdict_values(args[1:]), script=False)
            spec = f"{image_name} {image_spec}"
        else:
            class_name, part_id = args[:2]
            state_map = _format_optvalue(_mapdict_values(args[2:]), script=False)
            spec = f"{class_name} {part_id} {state_map}"
        opts = _format_optdict(kw, script)
    elif etype == "from":
        spec = args[0]
        if len(args) > 1:
            opts = (_format_optvalue(args[1], script),)

    if script and spec is not None:
        spec = f"{{{spec}}}"
        opts = " ".join(opts)
    return spec, opts


def _format_layoutlist(layout, indent=0, indent_size=2):
    script = []
    for elem, opts in layout:
        opts = opts or {}
        formatted_opts = " ".join(_format_optdict(opts, True, ("children",)))
        head = f"{' ' * indent}{elem}"
        if formatted_opts:
            head = f"{head} {formatted_opts}"
        if "children" in opts:
            script.append(f"{head} -children {{")
            indent += indent_size
            nested, indent = _format_layoutlist(opts["children"], indent, indent_size)
            script.append(nested)
            indent -= indent_size
            script.append(f"{' ' * indent}}}")
        else:
            script.append(head)
    return "\n".join(script), indent


def _script_from_settings(settings):
    script = []
    for name, opts in settings.items():
        if opts.get("configure"):
            formatted = " ".join(_format_optdict(opts["configure"], True))
            script.append(f"ttk::style configure {name} {formatted};")
        if opts.get("map"):
            formatted = " ".join(_format_mapdict(opts["map"], True))
            script.append(f"ttk::style map {name} {formatted};")
        if "layout" in opts:
            if not opts["layout"]:
                layout_text = "null"
            else:
                layout_text, _ = _format_layoutlist(opts["layout"])
            script.append(f"ttk::style layout {name} {{\n{layout_text}\n}}")
        if opts.get("element create"):
            eopts = opts["element create"]
            etype = eopts[0]
            argc = 1
            while argc < len(eopts) and not hasattr(eopts[argc], "items"):
                argc += 1
            elemargs = eopts[1:argc]
            elemkw = eopts[argc] if argc < len(eopts) and eopts[argc] else {}
            spec, formatted = _format_elemcreate(etype, True, *elemargs, **elemkw)
            script.append(
                f"ttk::style element create {name} {etype} {spec} {formatted}"
            )
    return "\n".join(script)


def _list_from_statespec(stuple):
    if isinstance(stuple, str):
        return stuple
    result = []
    it = iter(stuple)
    for state, val in zip(it, it):
        if hasattr(state, "typename"):
            state = str(state).split()
        elif isinstance(state, str):
            state = state.split()
        elif not isinstance(state, (tuple, list)):
            state = (state,)
        if hasattr(val, "typename"):
            val = str(val)
        result.append((*state, val))
    return result


def _list_from_layouttuple(tk, ltuple):
    layout_tuple = tk.splitlist(ltuple)
    res = []
    index = 0
    while index < len(layout_tuple):
        name = layout_tuple[index]
        opts = {}
        res.append((name, opts))
        index += 1
        while index < len(layout_tuple):
            opt, val = layout_tuple[index : index + 2]
            if not opt.startswith("-"):
                break
            opt = opt[1:]
            index += 2
            if opt == "children":
                val = _list_from_layouttuple(tk, val)
            opts[opt] = val
    return res


def _val_or_dict(tk, options, *args):
    formatted_options = _format_optdict(options)
    res = tk.call(*(args + formatted_options))
    if len(formatted_options) % 2:
        return res
    splitdict = getattr(_tkinter, "_splitdict", None)
    if callable(splitdict):
        return splitdict(tk, res, conv=_tclobj_to_py)
    return _split_pairs_to_dict(tk, res)


def _to_number(x):
    if isinstance(x, str):
        if "." in x:
            return float(x)
        return int(x)
    return x


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
    if isinstance(value, int):
        return value
    if isinstance(value, str) and value.lstrip("-").isdigit():
        return tk.getint(value)
    return value


def _event_from_subst_args(widget, event_args):
    args = _MOLT_TK_EVENT_SUBST_PARSE(getattr(widget, "_w", ""), event_args)
    if args is None:
        return None

    if isinstance(args, list):
        args = tuple(args)
    if not isinstance(args, tuple) or len(args) != len(_SUBST_FORMAT):
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
    if f not in ("", None):
        getboolean = getattr(widget.tk, "getboolean", None)
        if callable(getboolean):
            event.focus = getboolean(f)
    event.height = _event_int(widget.tk, h)
    event.keycode = _event_int(widget.tk, k)
    event.state = _event_int(widget.tk, s)
    event.time = _event_int(widget.tk, t)
    event.width = _event_int(widget.tk, w)
    event.x = _event_int(widget.tk, x)
    event.y = _event_int(widget.tk, y)
    event.char = a
    if e_send not in ("", None):
        getboolean = getattr(widget.tk, "getboolean", None)
        if callable(getboolean):
            event.send_event = getboolean(e_send)
    event.keysym = keysym
    event.keysym_num = _event_int(widget.tk, keysym_num)
    event.type = ev_type
    if isinstance(widget_path, str) and widget_path:
        event.widget = widget if widget_path == widget._w else widget_path
    else:
        event.widget = widget
    event.x_root = _event_int(widget.tk, x_root)
    event.y_root = _event_int(widget.tk, y_root)
    if delta in ("", None):
        event.delta = 0
    else:
        event.delta = _event_int(widget.tk, delta)
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
    if isinstance(value, (int, float)):
        return value
    if not isinstance(value, str):
        return value
    trimmed = value.strip()
    if not trimmed:
        return value
    try:
        if trimmed.startswith(("+0x", "-0x", "+0X", "-0X")):
            return int(trimmed, 16)
        if trimmed.startswith(("0x", "0X")):
            return int(trimmed, 16)
        if trimmed.startswith(("+0o", "-0o", "+0O", "-0O")):
            return int(trimmed, 8)
        if trimmed.startswith(("0o", "0O")):
            return int(trimmed, 8)
        if trimmed.startswith(("+0b", "-0b", "+0B", "-0B")):
            return int(trimmed, 2)
        if trimmed.startswith(("0b", "0B")):
            return int(trimmed, 2)
        return int(trimmed, 10)
    except ValueError:
        pass
    try:
        as_float = float(trimmed)
    except ValueError:
        return value
    if as_float == as_float and as_float not in (float("inf"), float("-inf")):
        return as_float
    return value


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

    def get(self):
        _require_gui_capability()
        return self.tk.call(self._w, "get")

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
        if hasattr(self, "_variable"):
            del self._variable
        if hasattr(self, "variable"):
            del self.variable
        super().destroy()


class Panedwindow(Widget):
    _widget_command = "ttk::panedwindow"

    def add(self, child, cnf=None, **kw):
        _require_gui_capability()
        opts = _normalize_options(cnf, **kw)
        return self.tk.call(self._w, "add", child, *opts)

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

    def panes(self):
        _require_gui_capability()
        return self.tk.splitlist(self.tk.call(self._w, "panes"))

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

    def set(self, value):
        _require_gui_capability()
        return self.tk.call(self._w, "set", value)


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
            trace_remove("write", self.__tracecb)
        if hasattr(self, "_variable"):
            del self._variable
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

    def get(self):
        _require_gui_capability()
        return self.tk.call(self._w, "get")

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

        command_name = _TK_TREEVIEW_TAG_BIND_REGISTER(
            self._tk_app,
            self._w,
            tagname,
            sequence,
            wrapped,
        )
        return command_name

    def tag_unbind(self, tagname, sequence, funcid=None):
        _require_gui_capability()
        if funcid is None:
            return self.tk.call(self._w, "tag", "bind", tagname, sequence, "")
        command_name = str(funcid)
        _TK_TREEVIEW_TAG_BIND_UNREGISTER(
            self._tk_app,
            self._w,
            tagname,
            sequence,
            command_name,
        )
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

    def xview(self, *args):
        _require_gui_capability()
        result = self.tk.call(self._w, "xview", *args)
        if args:
            return result
        return tuple(self.tk.getdouble(part) for part in self.tk.splitlist(result))

    def yview(self, *args):
        _require_gui_capability()
        result = self.tk.call(self._w, "yview", *args)
        if args:
            return result
        return tuple(self.tk.getdouble(part) for part in self.tk.splitlist(result))


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
        options = {}
        if isinstance(cnf, dict):
            options.update(cnf)
        if kw:
            options.update(kw)
        if query_opt is not None:
            if options:
                raise TypeError(
                    "configure() query_opt cannot be combined with update options"
                )
            options[query_opt] = None
        result = _val_or_dict(self.tk, options, "ttk::style", "configure", style)
        if result or query_opt:
            return result
        return None

    def map(self, style, query_opt=None, cnf=None, **kw):
        _require_gui_capability()
        options = {}
        if isinstance(cnf, dict):
            options.update(cnf)
        if kw:
            options.update(kw)
        if query_opt is not None:
            if options:
                raise TypeError(
                    "map() query_opt cannot be combined with update options"
                )
            options[query_opt] = None
        result = _val_or_dict(self.tk, options, "ttk::style", "map", style)
        if result or query_opt:
            return result
        return None

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
            return _list_from_layouttuple(
                self.tk, self.tk.call("ttk::style", "layout", style)
            )
        if layoutspec:
            layoutspec, _ = _format_layoutlist(layoutspec)
        return self.tk.call("ttk::style", "layout", style, layoutspec)

    def element_create(self, elementname, etype, *args):
        _require_gui_capability()
        spec, opts = _format_elemcreate(etype, False, *args)
        if spec is None:
            return self.tk.call(
                "ttk::style", "element", "create", elementname, etype, *args
            )
        return self.tk.call(
            "ttk::style", "element", "create", elementname, etype, spec, *opts
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
            if hasattr(settings, "items"):
                settings = _script_from_settings(settings)
            argv.extend(["-settings", settings])
        return self.tk.call(*argv)

    def theme_settings(self, themename, settings):
        _require_gui_capability()
        if hasattr(settings, "items"):
            settings = _script_from_settings(settings)
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
    raise AttributeError(f'module "{__name__}" has no attribute "{attr}"')
