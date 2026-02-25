"""Phase-0 intrinsic-backed `tkinter` wrapper surface.

The module intentionally exposes only a minimal Tk core while broader tkinter
lowering is in progress. All behavior routes through `_tkinter` intrinsics.
"""

import _tkinter as _phase0_tk
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has", globals())
_MOLT_TK_AVAILABLE = _require_intrinsic("molt_tk_available", globals())


NO_VALUE = object()
_default_root = None
_variable_serial = 0
_command_serial = 0


def _require_phase0_callable(attr):
    try:
        value = getattr(_phase0_tk, attr)
    except Exception as exc:  # noqa: BLE001
        raise RuntimeError(
            f"tkinter requires _tkinter.{attr} in the Phase-0 intrinsic surface"
        ) from exc
    if not callable(value):
        raise RuntimeError(
            f"tkinter requires callable _tkinter.{attr} in the Phase-0 intrinsic surface"
        )
    return value


_TK_AVAILABLE = _require_phase0_callable("tk_available")
_HAS_GUI_CAPABILITY = _require_phase0_callable("has_gui_capability")
_HAS_PROCESS_SPAWN_CAPABILITY = _require_phase0_callable("has_process_spawn_capability")
_TK_CREATE = _require_phase0_callable("create")
_TK_MAINLOOP = _require_phase0_callable("mainloop")
_TK_DO_ONE_EVENT = _require_phase0_callable("dooneevent")
_TK_QUIT = _require_phase0_callable("quit")
_TK_AFTER = _require_phase0_callable("after")
_TK_CALL = _require_phase0_callable("call")
_TK_BIND_COMMAND = _require_phase0_callable("bind_command")
_TK_DESTROY_WIDGET = _require_phase0_callable("destroy_widget")
_TK_LAST_ERROR = _require_phase0_callable("last_error")

TclError = _phase0_tk.TclError


def _has_any_capability(*names):
    return any(bool(_MOLT_CAPABILITIES_HAS(name)) for name in names)


def _require_gui_window_capability():
    if not bool(_HAS_GUI_CAPABILITY()) and not _has_any_capability("gui.window", "gui"):
        raise PermissionError("missing gui.window capability")


def _require_process_spawn_capability():
    if not bool(_HAS_PROCESS_SPAWN_CAPABILITY()) and not _has_any_capability(
        "process.spawn", "process"
    ):
        raise PermissionError("missing process.spawn capability")


def _normalize_option_name(name):
    return name if name.startswith("-") else f"-{name}"


def _normalize_tk_options(cnf=None, **kw):
    if cnf is not None and not isinstance(cnf, dict):
        raise TypeError("tkinter config must be a dict or None")
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


def _normalize_delay_ms(delay_ms):
    try:
        return int(delay_ms)
    except Exception as exc:  # noqa: BLE001
        raise TypeError("after delay must be an integer") from exc


def _normalize_bind_add(add):
    if add in (None, "", False, 0):
        return ""
    if add in (True, "+", 1):
        return "+"
    raise TypeError("bind add must be one of: None, '', False, True, or '+'")


def _normalize_bind_target(target):
    if hasattr(target, "_w"):
        return str(target._w)
    return str(target)


def _normalize_trace_mode(mode):
    if isinstance(mode, (tuple, list)):
        return " ".join(str(part) for part in mode)
    return str(mode)


def _next_command_name(prefix):
    global _command_serial
    name = f"::__molt_tkinter_{prefix}_{_command_serial}"
    _command_serial += 1
    return name


def _pop_after_command(root, token):
    key = str(token)
    command_name = root._after_tokens.pop(key, None)
    if command_name is None:
        return None
    aliases = [
        alias
        for alias, mapped in tuple(root._after_tokens.items())
        if mapped == command_name
    ]
    for alias in aliases:
        root._after_tokens.pop(alias, None)
    return command_name


def _set_default_root(root):
    global _default_root
    if _default_root is None:
        _default_root = root


def _clear_default_root(root):
    global _default_root
    if _default_root is root:
        _default_root = None


def _get_default_root():
    if _default_root is None:
        return Tk()
    return _default_root


def _next_variable_name():
    global _variable_serial
    name = f"PY_VAR{_variable_serial}"
    _variable_serial += 1
    return name


class Event:
    """Minimal tkinter event object placeholder for bind callbacks."""

    pass


class Misc:
    """Shared Phase-0 object helpers for Tk and widgets."""

    def call(self, *argv):
        _require_gui_window_capability()
        return _TK_CALL(self._tk_app, *argv)

    def _call_widget(self, command, *args):
        return self.call(self._w, command, *args)

    def _split_ints(self, value):
        return tuple(self.getint(part) for part in self.splitlist(value))

    def _register_command(self, prefix, callback):
        if not callable(callback):
            raise TypeError("callback must be callable")
        command_name = _next_command_name(prefix)
        self.createcommand(command_name, callback)
        return command_name

    def _release_command(self, command_name):
        if command_name is None:
            return None
        name = str(command_name)
        try:
            self.deletecommand(name)
        except Exception:  # noqa: BLE001
            root = getattr(self, "tk", None)
            if root is not None and hasattr(root, "_registered_commands"):
                root._registered_commands.discard(name)
        return None

    def configure(self, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "configure() option query cannot be combined with update options"
                )
            return self._call_widget("configure", _normalize_option_name(cnf))
        if cnf is None and not kw:
            return self._call_widget("configure")
        return self._call_widget("configure", *_normalize_tk_options(cnf, **kw))

    config = configure

    def cget(self, key):
        return self._call_widget("cget", _normalize_option_name(key))

    def __getitem__(self, key):
        return self.cget(key)

    def __setitem__(self, key, value):
        self.configure(**{key: value})

    def keys(self):
        configured = self._call_widget("configure")
        keys = []
        if isinstance(configured, (tuple, list)):
            for entry in configured:
                if not isinstance(entry, (tuple, list)):
                    continue
                if not entry:
                    continue
                key = entry[0]
                if not isinstance(key, str):
                    continue
                keys.append(key[1:] if key.startswith("-") else key)
            if keys:
                return keys
        return [str(item).lstrip("-") for item in self.splitlist(configured)]

    def mainloop(self, n=0):
        del n
        _require_gui_window_capability()
        _TK_MAINLOOP(self._tk_app)

    def dooneevent(self, flags=0):
        _require_gui_window_capability()
        return bool(_TK_DO_ONE_EVENT(self._tk_app, flags))

    def quit(self):
        _require_gui_window_capability()
        _TK_QUIT(self._tk_app)

    def update(self):
        return self.call("update")

    def update_idletasks(self):
        return self.call("update", "idletasks")

    def after(self, delay_ms, callback=None, *args):
        _require_gui_window_capability()
        delay = _normalize_delay_ms(delay_ms)
        if callback is None:
            return self.call("after", delay)
        if not callable(callback):
            raise TypeError("after callback must be callable")
        if args:

            def wrapped():
                return callback(*args)

            return _TK_AFTER(self._tk_app, delay, wrapped)
        return _TK_AFTER(self._tk_app, delay, callback)

    def after_idle(self, callback, *args):
        _require_gui_window_capability()
        if not callable(callback):
            raise TypeError("after_idle callback must be callable")

        state = {"command_name": None, "released": False}

        def wrapped():
            if state["released"]:
                return None
            state["released"] = True
            command_name = state["command_name"]
            if command_name is not None:
                _pop_after_command(self.tk, command_name)
                self._release_command(command_name)
            return callback(*args)

        command_name = self._register_command("after_idle", wrapped)
        state["command_name"] = command_name
        try:
            token = self.call("after", "idle", command_name)
        except Exception:
            state["released"] = True
            self._release_command(command_name)
            raise
        if not state["released"]:
            self.tk._after_tokens[str(token)] = command_name
            self.tk._after_tokens[command_name] = command_name
        return token

    def after_cancel(self, identifier):
        _require_gui_window_capability()
        if identifier is None:
            return None

        delete_timer = getattr(identifier, "deletetimerhandler", None)
        if callable(delete_timer):
            delete_timer()
            return None

        token = getattr(identifier, "_token", identifier)
        command_name = _pop_after_command(self.tk, token)
        try:
            self.call("after", "cancel", token)
        except Exception:  # noqa: BLE001
            # Keep cancellation idempotent across already-fired/canceled handles.
            pass
        if command_name is not None:
            self._release_command(command_name)
        return None

    def bind_command(self, name, callback):
        _require_gui_window_capability()
        if not callable(callback):
            raise TypeError("bind_command callback must be callable")
        _TK_BIND_COMMAND(self._tk_app, name, callback)

    def createcommand(self, name, callback):
        _require_gui_window_capability()
        _phase0_tk.createcommand(self._tk_app, name, callback)
        root = getattr(self, "tk", None)
        if root is not None and hasattr(root, "_registered_commands"):
            root._registered_commands.add(str(name))

    def deletecommand(self, name):
        value = _phase0_tk.deletecommand(self._tk_app, name)
        root = getattr(self, "tk", None)
        if root is not None and hasattr(root, "_registered_commands"):
            root._registered_commands.discard(str(name))
        return value

    def _bind(self, target, sequence=None, func=None, add=None):
        _require_gui_window_capability()
        target_name = _normalize_bind_target(target)

        if func is None:
            if sequence is None:
                return self.call("bind", target_name)
            return self.call("bind", target_name, sequence)

        if sequence is None:
            raise TypeError("bind sequence must not be None when callback is provided")

        add_prefix = _normalize_bind_add(add)

        if isinstance(func, str):
            script = f"{add_prefix}{func}" if add_prefix else func
            self.call("bind", target_name, sequence, script)
            return func

        if not callable(func):
            raise TypeError("bind callback must be callable")

        widget = self

        def wrapped(*event_args):
            if event_args:
                return func(*event_args)
            event = Event()
            event.widget = widget
            return func(event)

        command_name = self._register_command("bind", wrapped)
        script = f"{add_prefix}{command_name}" if add_prefix else command_name
        try:
            self.call("bind", target_name, sequence, script)
        except Exception:
            self._release_command(command_name)
            raise
        return command_name

    def _unbind(self, target, sequence, funcid=None):
        _require_gui_window_capability()
        if sequence is None:
            raise TypeError("unbind sequence must not be None")

        target_name = _normalize_bind_target(target)
        if funcid is None:
            return self.call("bind", target_name, sequence, "")

        command_name = str(funcid)
        try:
            script = self.call("bind", target_name, sequence)
        except Exception:  # noqa: BLE001
            script = ""

        replacement = ""
        if isinstance(script, str) and script:
            kept = []
            for line in script.split("\n"):
                candidate = line.strip()
                if not candidate:
                    continue
                if candidate.lstrip("+") == command_name:
                    continue
                kept.append(candidate)
            replacement = "\n".join(kept)

        self.call("bind", target_name, sequence, replacement)
        self._release_command(command_name)
        return None

    def bind(self, sequence=None, func=None, add=None):
        return self._bind(self._w, sequence, func, add)

    def unbind(self, sequence, funcid=None):
        return self._unbind(self._w, sequence, funcid)

    def bind_all(self, sequence=None, func=None, add=None):
        return self._bind("all", sequence, func, add)

    def unbind_all(self, sequence):
        return self._unbind("all", sequence)

    def bind_class(self, class_name, sequence=None, func=None, add=None):
        return self._bind(class_name, sequence, func, add)

    def unbind_class(self, class_name, sequence):
        return self._unbind(class_name, sequence)

    def bindtags(self, tag_list=None):
        if tag_list is None:
            return self.splitlist(self.call("bindtags", self._w))
        if isinstance(tag_list, (tuple, list)):
            return self.call("bindtags", self._w, tuple(str(tag) for tag in tag_list))
        return self.call("bindtags", self._w, tag_list)

    def event_add(self, virtual, *sequences):
        return self.call("event", "add", virtual, *sequences)

    def event_delete(self, virtual, *sequences):
        return self.call("event", "delete", virtual, *sequences)

    def event_generate(self, sequence, **kw):
        return self.call(
            "event",
            "generate",
            self._w,
            sequence,
            *_normalize_tk_options(None, **kw),
        )

    def event_info(self, virtual=None):
        if virtual is None:
            return self.splitlist(self.call("event", "info"))
        return self.splitlist(self.call("event", "info", virtual))

    def destroy(self):
        _require_gui_window_capability()
        _TK_DESTROY_WIDGET(self._tk_app, self._w)

    def last_error(self):
        _require_gui_window_capability()
        return _TK_LAST_ERROR(self._tk_app)

    def getboolean(self, value):
        return _phase0_tk.getboolean(value)

    def getint(self, value):
        return _phase0_tk.getint(value)

    def getdouble(self, value):
        return _phase0_tk.getdouble(value)

    def splitlist(self, value):
        return _phase0_tk.splitlist(value)

    def getvar(self, name="PY_VAR"):
        return _phase0_tk.getvar(self._tk_app, name)

    def setvar(self, name="PY_VAR", value="1"):
        return _phase0_tk.setvar(self._tk_app, name, value)

    def unsetvar(self, name="PY_VAR"):
        return _phase0_tk.unsetvar(self._tk_app, name)

    def globalgetvar(self, name="PY_VAR"):
        return _phase0_tk.globalgetvar(self._tk_app, name)

    def globalsetvar(self, name="PY_VAR", value="1"):
        return _phase0_tk.globalsetvar(self._tk_app, name, value)

    def globalunsetvar(self, name="PY_VAR"):
        return _phase0_tk.globalunsetvar(self._tk_app, name)

    def wait_variable(self, name="PY_VAR"):
        variable_name = name._name if hasattr(name, "_name") else name
        return self.call("tkwait", "variable", variable_name)

    waitvar = wait_variable

    def wait_window(self, window=None):
        target = self._w if window is None else _normalize_bind_target(window)
        return self.call("tkwait", "window", target)

    def wait_visibility(self, window=None):
        target = self._w if window is None else _normalize_bind_target(window)
        return self.call("tkwait", "visibility", target)

    def focus_set(self):
        return self.call("focus", self._w)

    def focus_force(self):
        return self.call("focus", "-force", self._w)

    def focus_get(self):
        return self.call("focus")

    def focus_lastfor(self):
        return self.call("focus", "-lastfor", self._w)

    def tk_focusNext(self):
        return self.call("tk_focusNext", self._w)

    def tk_focusPrev(self):
        return self.call("tk_focusPrev", self._w)

    def tk_focusFollowsMouse(self):
        return self.call("tk_focusFollowsMouse")

    def grab_set(self):
        return self.call("grab", "set", self._w)

    def grab_set_global(self):
        return self.call("grab", "set", "-global", self._w)

    def grab_release(self):
        return self.call("grab", "release", self._w)

    def grab_current(self):
        return self.call("grab", "current", self._w)

    def grab_status(self):
        status = self.call("grab", "status", self._w)
        return None if status == "" else status

    def bell(self, displayof=None):
        if displayof is None:
            return self.call("bell")
        return self.call("bell", "-displayof", _normalize_bind_target(displayof))

    def clipboard_clear(self, **kw):
        return self.call("clipboard", "clear", *_normalize_tk_options(None, **kw))

    def clipboard_append(self, string, **kw):
        return self.call(
            "clipboard",
            "append",
            *_normalize_tk_options(None, **kw),
            "--",
            string,
        )

    def clipboard_get(self, **kw):
        return self.call("clipboard", "get", *_normalize_tk_options(None, **kw))

    def selection_get(self, **kw):
        return self.call("selection", "get", *_normalize_tk_options(None, **kw))

    def selection_clear(self, **kw):
        return self.call("selection", "clear", *_normalize_tk_options(None, **kw))

    def pack_configure(self, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "pack_configure() option query cannot be combined with updates"
                )
            return self.call("pack", "configure", self._w, _normalize_option_name(cnf))
        return self.call(
            "pack", "configure", self._w, *_normalize_tk_options(cnf, **kw)
        )

    pack = pack_configure

    def pack_forget(self):
        return self.call("pack", "forget", self._w)

    forget = pack_forget

    def pack_info(self):
        return self.call("pack", "info", self._w)

    def pack_propagate(self, flag=NO_VALUE):
        if flag is NO_VALUE:
            return self.call("pack", "propagate", self._w)
        return self.call("pack", "propagate", self._w, int(bool(flag)))

    def pack_slaves(self):
        return self.splitlist(self.call("pack", "slaves", self._w))

    pack_children = pack_slaves

    def grid_configure(self, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "grid_configure() option query cannot be combined with updates"
                )
            return self.call("grid", "configure", self._w, _normalize_option_name(cnf))
        return self.call(
            "grid", "configure", self._w, *_normalize_tk_options(cnf, **kw)
        )

    grid = grid_configure

    def grid_forget(self):
        return self.call("grid", "forget", self._w)

    def grid_remove(self):
        return self.call("grid", "remove", self._w)

    def grid_info(self):
        return self.call("grid", "info", self._w)

    def grid_propagate(self, flag=NO_VALUE):
        if flag is NO_VALUE:
            return self.call("grid", "propagate", self._w)
        return self.call("grid", "propagate", self._w, int(bool(flag)))

    def grid_bbox(self, column=None, row=None, col2=None, row2=None):
        args = [self._w]
        if column is not None:
            args.append(column)
        if row is not None:
            args.append(row)
        if col2 is not None:
            args.append(col2)
        if row2 is not None:
            args.append(row2)
        return self._split_ints(self.call("grid", "bbox", *args))

    def grid_location(self, x, y):
        return self._split_ints(self.call("grid", "location", self._w, x, y))

    def grid_size(self):
        return self._split_ints(self.call("grid", "size", self._w))

    def grid_columnconfigure(self, index, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "columnconfigure() option query cannot be combined with updates"
                )
            return self.call(
                "grid",
                "columnconfigure",
                self._w,
                index,
                _normalize_option_name(cnf),
            )
        return self.call(
            "grid",
            "columnconfigure",
            self._w,
            index,
            *_normalize_tk_options(cnf, **kw),
        )

    def grid_rowconfigure(self, index, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "rowconfigure() option query cannot be combined with updates"
                )
            return self.call(
                "grid",
                "rowconfigure",
                self._w,
                index,
                _normalize_option_name(cnf),
            )
        return self.call(
            "grid",
            "rowconfigure",
            self._w,
            index,
            *_normalize_tk_options(cnf, **kw),
        )

    columnconfigure = grid_columnconfigure
    rowconfigure = grid_rowconfigure

    def grid_slaves(self, row=None, column=None):
        args = [self._w]
        if row is not None:
            args.extend(("-row", row))
        if column is not None:
            args.extend(("-column", column))
        return self.splitlist(self.call("grid", "slaves", *args))

    def place_configure(self, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "place_configure() option query cannot be combined with updates"
                )
            return self.call("place", "configure", self._w, _normalize_option_name(cnf))
        return self.call(
            "place", "configure", self._w, *_normalize_tk_options(cnf, **kw)
        )

    place = place_configure

    def place_forget(self):
        return self.call("place", "forget", self._w)

    def place_info(self):
        return self.call("place", "info", self._w)

    def place_slaves(self):
        return self.splitlist(self.call("place", "slaves", self._w))

    def lift(self, above_this=None):
        if above_this is None:
            return self.call("raise", self._w)
        return self.call("raise", self._w, _normalize_bind_target(above_this))

    tkraise = lift

    def lower(self, below_this=None):
        if below_this is None:
            return self.call("lower", self._w)
        return self.call("lower", self._w, _normalize_bind_target(below_this))

    def winfo_children(self):
        return self.splitlist(self.call("winfo", "children", self._w))

    def winfo_exists(self):
        return bool(self.getint(self.call("winfo", "exists", self._w)))

    def winfo_ismapped(self):
        return bool(self.getint(self.call("winfo", "ismapped", self._w)))

    def winfo_viewable(self):
        return bool(self.getint(self.call("winfo", "viewable", self._w)))

    def winfo_manager(self):
        return self.call("winfo", "manager", self._w)

    def winfo_class(self):
        return self.call("winfo", "class", self._w)

    def winfo_name(self):
        return self.call("winfo", "name", self._w)

    def winfo_parent(self):
        return self.call("winfo", "parent", self._w)

    def winfo_toplevel(self):
        return self.call("winfo", "toplevel", self._w)

    def winfo_id(self):
        return self.getint(self.call("winfo", "id", self._w))

    def winfo_width(self):
        return self.getint(self.call("winfo", "width", self._w))

    def winfo_height(self):
        return self.getint(self.call("winfo", "height", self._w))

    def winfo_reqwidth(self):
        return self.getint(self.call("winfo", "reqwidth", self._w))

    def winfo_reqheight(self):
        return self.getint(self.call("winfo", "reqheight", self._w))

    def winfo_x(self):
        return self.getint(self.call("winfo", "x", self._w))

    def winfo_y(self):
        return self.getint(self.call("winfo", "y", self._w))

    def winfo_rootx(self):
        return self.getint(self.call("winfo", "rootx", self._w))

    def winfo_rooty(self):
        return self.getint(self.call("winfo", "rooty", self._w))

    def winfo_screenwidth(self):
        return self.getint(self.call("winfo", "screenwidth", self._w))

    def winfo_screenheight(self):
        return self.getint(self.call("winfo", "screenheight", self._w))

    def winfo_pointerx(self):
        return self.getint(self.call("winfo", "pointerx", self._w))

    def winfo_pointery(self):
        return self.getint(self.call("winfo", "pointery", self._w))

    def winfo_pointerxy(self):
        return self._split_ints(self.call("winfo", "pointerxy", self._w))

    def winfo_rgb(self, color):
        return self._split_ints(self.call("winfo", "rgb", self._w, color))

    def winfo_atom(self, name):
        return self.getint(self.call("winfo", "atom", name))

    def winfo_atomname(self, atom_id):
        return self.call("winfo", "atomname", atom_id)

    def winfo_containing(self, root_x, root_y, displayof=None):
        if displayof is None:
            return self.call("winfo", "containing", root_x, root_y)
        return self.call(
            "winfo",
            "containing",
            "-displayof",
            _normalize_bind_target(displayof),
            root_x,
            root_y,
        )


class Tk(Misc):
    """Phase-0 root window wrapper backed by `_tkinter` intrinsics."""

    def __init__(
        self,
        screenName=None,
        baseName=None,
        className="Tk",
        useTk=True,
        sync=False,
        use=None,
    ):
        _require_gui_window_capability()
        _require_process_spawn_capability()
        options = {
            "screenName": screenName,
            "baseName": baseName,
            "className": className,
            "useTk": bool(useTk),
            "sync": bool(sync),
            "use": use,
        }
        self._tk_app = _TK_CREATE(options=options)
        self._registered_commands = set()
        self._after_tokens = {}
        self._protocol_commands = {}
        self.children = {}
        self._w = "."
        self.tk = self
        self._widget_serial = 0
        _set_default_root(self)

    def _next_widget_path(self, widget_command):
        base = widget_command.replace("::", "_").replace("-", "_")
        if not base:
            base = "widget"
        self._widget_serial += 1
        return f".!{base}{self._widget_serial}"

    def _wm_call(self, command, *args):
        return self.call("wm", command, self._w, *args)

    def _purge_registered_commands(self):
        for command_name in list(self._registered_commands):
            try:
                _phase0_tk.deletecommand(self._tk_app, command_name)
            except Exception:  # noqa: BLE001
                pass
        self._registered_commands.clear()
        self._after_tokens.clear()
        self._protocol_commands.clear()

    def wm_title(self, string=None):
        if string is None:
            return self._wm_call("title")
        return self._wm_call("title", string)

    title = wm_title

    def wm_geometry(self, new_geometry=None):
        if new_geometry is None:
            return self._wm_call("geometry")
        return self._wm_call("geometry", new_geometry)

    geometry = wm_geometry

    def wm_state(self, new_state=None):
        if new_state is None:
            return self._wm_call("state")
        return self._wm_call("state", new_state)

    state = wm_state

    def wm_attributes(self, *args, **kw):
        if args and kw:
            raise TypeError(
                "wm_attributes() cannot mix positional arguments and kwargs"
            )
        if kw:
            flat = []
            for key, value in kw.items():
                flat.append(_normalize_option_name(str(key)))
                flat.append(value)
            args = tuple(flat)
        if not args:
            return self._wm_call("attributes")
        return self._wm_call("attributes", *args)

    attributes = wm_attributes

    def wm_resizable(self, width=None, height=None):
        if width is None and height is None:
            values = self.splitlist(self._wm_call("resizable"))
            if len(values) >= 2:
                return (self.getboolean(values[0]), self.getboolean(values[1]))
            return tuple(self.getboolean(value) for value in values)
        if width is None or height is None:
            raise TypeError("wm_resizable() requires both width and height")
        return self._wm_call("resizable", int(bool(width)), int(bool(height)))

    resizable = wm_resizable

    def wm_protocol(self, name=None, func=None):
        if name is None:
            return self._wm_call("protocol")

        previous = self._protocol_commands.pop(name, None)
        if previous is not None:
            self._release_command(previous)

        if func is None:
            return self._wm_call("protocol", name)

        if isinstance(func, str):
            return self._wm_call("protocol", name, func)

        if not callable(func):
            raise TypeError("wm_protocol callback must be callable")

        command_name = self._register_command("wm_protocol", func)
        try:
            self._wm_call("protocol", name, command_name)
        except Exception:
            self._release_command(command_name)
            raise
        self._protocol_commands[name] = command_name
        return command_name

    protocol = wm_protocol

    def wm_iconify(self):
        return self._wm_call("iconify")

    iconify = wm_iconify

    def wm_deiconify(self):
        return self._wm_call("deiconify")

    deiconify = wm_deiconify

    def wm_withdraw(self):
        return self._wm_call("withdraw")

    withdraw = wm_withdraw

    def wm_minsize(self, width=None, height=None):
        if width is None and height is None:
            return self._split_ints(self._wm_call("minsize"))
        if width is None or height is None:
            raise TypeError("wm_minsize() requires both width and height")
        return self._wm_call("minsize", width, height)

    minsize = wm_minsize

    def wm_maxsize(self, width=None, height=None):
        if width is None and height is None:
            return self._split_ints(self._wm_call("maxsize"))
        if width is None or height is None:
            raise TypeError("wm_maxsize() requires both width and height")
        return self._wm_call("maxsize", width, height)

    maxsize = wm_maxsize

    def wm_overrideredirect(self, boolean=NO_VALUE):
        if boolean is NO_VALUE:
            return self.getboolean(self._wm_call("overrideredirect"))
        return self._wm_call("overrideredirect", int(bool(boolean)))

    overrideredirect = wm_overrideredirect

    def wm_transient(self, master=None):
        if master is None:
            return self._wm_call("transient")
        return self._wm_call("transient", _normalize_bind_target(master))

    transient = wm_transient

    def wm_iconname(self, new_name=None):
        if new_name is None:
            return self._wm_call("iconname")
        return self._wm_call("iconname", new_name)

    iconname = wm_iconname

    def destroy(self):
        self._purge_registered_commands()
        try:
            super().destroy()
        finally:
            _clear_default_root(self)

    def __str__(self):
        return self._w


class Widget(Misc):
    """Phase-0 widget shell used by tkinter/ttk wrappers."""

    def __init__(self, master, widget_command, cnf=None, **kw):
        parent = _get_default_root() if master is None else master
        if not isinstance(parent, Misc):
            raise TypeError("widget master must be a tkinter widget or root")
        root = parent.tk
        self.master = parent
        self.tk = root
        self._tk_app = root._tk_app
        self._w = root._next_widget_path(widget_command)
        self.children = {}
        if hasattr(parent, "children"):
            parent.children[self._w] = self
        argv = [widget_command, self._w]
        argv.extend(_normalize_tk_options(cnf, **kw))
        self.tk.call(*argv)

    def destroy(self):
        try:
            super().destroy()
        finally:
            if hasattr(self.master, "children"):
                self.master.children.pop(self._w, None)

    def __str__(self):
        return self._w


class _CoreWidget(Widget):
    _widget_command = "widget"

    def __init__(self, master=None, cnf=None, **kw):
        super().__init__(master, self._widget_command, cnf, **kw)


class Button(_CoreWidget):
    _widget_command = "button"


class Label(_CoreWidget):
    _widget_command = "label"


class Entry(_CoreWidget):
    _widget_command = "entry"


class Frame(_CoreWidget):
    _widget_command = "frame"


class Canvas(_CoreWidget):
    _widget_command = "canvas"


class Text(_CoreWidget):
    _widget_command = "text"


class Toplevel(_CoreWidget):
    _widget_command = "toplevel"


class Listbox(_CoreWidget):
    _widget_command = "listbox"


class Menu(_CoreWidget):
    _widget_command = "menu"


class Scrollbar(_CoreWidget):
    _widget_command = "scrollbar"


class Checkbutton(_CoreWidget):
    _widget_command = "checkbutton"


class Radiobutton(_CoreWidget):
    _widget_command = "radiobutton"


class Spinbox(_CoreWidget):
    _widget_command = "spinbox"


class Scale(_CoreWidget):
    _widget_command = "scale"


class PanedWindow(_CoreWidget):
    _widget_command = "panedwindow"


class LabelFrame(_CoreWidget):
    _widget_command = "labelframe"


class Message(_CoreWidget):
    _widget_command = "message"


class Variable:
    _default = ""

    def __init__(self, master=None, value=None, name=None):
        parent = _get_default_root() if master is None else master
        if not isinstance(parent, Misc):
            raise TypeError("variable master must be a tkinter widget or root")
        self._root = parent.tk
        self._tk = parent.tk
        self._trace_callbacks = {}
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

    def __str__(self):
        return self._name

    def set(self, value):
        return self._tk.setvar(self._name, value)

    initialize = set

    def get(self):
        return self._tk.getvar(self._name)

    def trace_add(self, mode, callback):
        if not callable(callback):
            raise TypeError("trace callback must be callable")

        mode_name = _normalize_trace_mode(mode)

        def wrapped(*args):
            if args:
                return callback(*args)
            return callback(self._name, "", mode_name)

        command_name = self._root._register_command("trace", wrapped)
        try:
            self._tk.call(
                "trace",
                "add",
                "variable",
                self._name,
                mode_name,
                command_name,
            )
        except Exception:
            self._root._release_command(command_name)
            raise
        self._trace_callbacks[command_name] = mode_name
        return command_name

    def trace_remove(self, mode, cbname):
        mode_name = _normalize_trace_mode(mode)
        command_name = str(cbname)
        self._tk.call(
            "trace",
            "remove",
            "variable",
            self._name,
            mode_name,
            command_name,
        )
        self._trace_callbacks.pop(command_name, None)
        self._root._release_command(command_name)

    def trace_info(self):
        return self._tk.splitlist(
            self._tk.call("trace", "info", "variable", self._name)
        )

    def trace(self, mode, callback):
        return self.trace_add(mode, callback)

    def trace_variable(self, mode, callback):
        return self.trace_add(mode, callback)

    def trace_vdelete(self, mode, cbname):
        return self.trace_remove(mode, cbname)

    def trace_vinfo(self):
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


def mainloop(n=0):
    _get_default_root().mainloop(n)


def dooneevent(flags=0):
    return _get_default_root().dooneevent(flags)


def quit():
    _get_default_root().quit()


def after(delay_ms, callback=None, *args):
    return _get_default_root().after(delay_ms, callback, *args)


def after_idle(callback, *args):
    return _get_default_root().after_idle(callback, *args)


def after_cancel(identifier):
    return _get_default_root().after_cancel(identifier)


def getvar(name="PY_VAR"):
    return _get_default_root().getvar(name)


def setvar(name="PY_VAR", value="1"):
    return _get_default_root().setvar(name, value)


def unsetvar(name="PY_VAR"):
    return _get_default_root().unsetvar(name)


def globalgetvar(name="PY_VAR"):
    return _get_default_root().globalgetvar(name)


def globalsetvar(name="PY_VAR", value="1"):
    return _get_default_root().globalsetvar(name, value)


def globalunsetvar(name="PY_VAR"):
    return _get_default_root().globalunsetvar(name)


def tk_available():
    return bool(_MOLT_TK_AVAILABLE()) and bool(_TK_AVAILABLE())


def getboolean(value):
    return _phase0_tk.getboolean(value)


def getint(value):
    return _phase0_tk.getint(value)


def getdouble(value):
    return _phase0_tk.getdouble(value)


def splitlist(value):
    return _phase0_tk.splitlist(value)


__all__ = [
    "BooleanVar",
    "Button",
    "Canvas",
    "Checkbutton",
    "DoubleVar",
    "Entry",
    "Event",
    "Frame",
    "IntVar",
    "Label",
    "LabelFrame",
    "Listbox",
    "Menu",
    "Message",
    "Misc",
    "PanedWindow",
    "Radiobutton",
    "Scale",
    "Scrollbar",
    "Spinbox",
    "StringVar",
    "TclError",
    "Text",
    "Tk",
    "Toplevel",
    "Variable",
    "Widget",
    "after",
    "after_cancel",
    "after_idle",
    "dooneevent",
    "getboolean",
    "getdouble",
    "getint",
    "getvar",
    "globalgetvar",
    "globalsetvar",
    "globalunsetvar",
    "mainloop",
    "quit",
    "setvar",
    "splitlist",
    "tk_available",
    "unsetvar",
]


def __getattr__(attr):
    raise AttributeError(
        f'module "{__name__}" has no attribute "{attr}"; '
        "only the Phase-0 tkinter core surface is implemented."
    )
