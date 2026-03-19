"""Intrinsic-backed `tkinter` wrapper surface."""

import re
import sys
import warnings

# On WASM targets without GUI support, fail at import time like CPython.
if sys.platform in ("emscripten", "wasi"):
    raise ImportError("No module named '_tkinter'")

import _tkinter as _tkimpl
from _intrinsics import require_intrinsic as _require_intrinsic
from .constants import *  # noqa: F403

import enum as _enum

_EventTypeBase = _enum.Enum

def _lazy_intrinsic(name):
    def _call(*args, **kwargs):
        return _require_intrinsic(name)(*args, **kwargs)

    return _call


_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_MOLT_TK_AVAILABLE = _require_intrinsic("molt_tk_available")
_MOLT_TK_EVENT_SUBST_PARSE = _require_intrinsic("molt_tk_event_subst_parse")
_molt_tk_event_int = _require_intrinsic("molt_tk_event_int")
_molt_tk_event_build_from_args = _require_intrinsic(
    "molt_tk_event_build_from_args")
_molt_tk_event_state_decode = _require_intrinsic(
    "molt_tk_event_state_decode")
_molt_tk_splitdict = _require_intrinsic("molt_tk_splitdict")
_molt_tk_flatten_args = _require_intrinsic("molt_tk_flatten_args")
_molt_tk_cnfmerge = _require_intrinsic("molt_tk_cnfmerge")
_molt_tk_normalize_option = _require_intrinsic(
    "molt_tk_normalize_option")
_molt_tk_normalize_delay_ms = _require_intrinsic(
    "molt_tk_normalize_delay_ms")


NO_VALUE = object()
_support_default_root = True
_default_root = None
_variable_serial = 0
_command_serial = 0
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
_MAGIC_RE = re.compile(r"([\\{}])")
_SPACE_RE = re.compile(r"([\s])", re.ASCII)


def _require_tk_callable(attr):
    def _call(*args, **kwargs):
        value = getattr(_tkimpl, attr, None)
        if value is None:
            raise RuntimeError(
                f"tkinter requires _tkinter.{attr} in the intrinsic runtime surface"
            )
        if not callable(value):
            raise RuntimeError(
                f"tkinter requires callable _tkinter.{attr} in the intrinsic runtime surface"
            )
        return value(*args, **kwargs)

    return _call


_TK_AVAILABLE = _require_tk_callable("tk_available")
_HAS_GUI_CAPABILITY = _require_tk_callable("has_gui_capability")
_HAS_PROCESS_SPAWN_CAPABILITY = _require_tk_callable("has_process_spawn_capability")
_TK_CREATE = _require_tk_callable("create")
_TK_MAINLOOP = _require_tk_callable("mainloop")
_TK_DO_ONE_EVENT = _require_tk_callable("dooneevent")
_TK_QUIT = _require_tk_callable("quit")
_TK_AFTER = _require_tk_callable("after")
_TK_CALL = _require_tk_callable("call")
_TK_BIND_COMMAND = _require_tk_callable("bind_command")
_TK_BIND_REGISTER = _require_tk_callable("bind_register")
_TK_BIND_UNREGISTER = _require_tk_callable("bind_unregister")
_TK_WIDGET_BIND_REGISTER = _require_tk_callable("widget_bind_register")
_TK_WIDGET_BIND_UNREGISTER = _require_tk_callable("widget_bind_unregister")
_TK_TEXT_TAG_BIND_REGISTER = _require_tk_callable("text_tag_bind_register")
_TK_TEXT_TAG_BIND_UNREGISTER = _require_tk_callable("text_tag_bind_unregister")
_TK_DESTROY_WIDGET = _require_tk_callable("destroy_widget")
_TK_LAST_ERROR = _require_tk_callable("last_error")
_TK_TRACE_ADD = _require_tk_callable("trace_add")
_TK_TRACE_REMOVE = _require_tk_callable("trace_remove")
_TK_TRACE_CLEAR = _require_tk_callable("trace_clear")
_TK_TRACE_INFO = _require_tk_callable("trace_info")
_TK_WAIT_VARIABLE = _require_tk_callable("wait_variable")
_TK_WAIT_WINDOW = _require_tk_callable("wait_window")
_TK_WAIT_VISIBILITY = _require_tk_callable("wait_visibility")

TclError = _tkimpl.TclError
wantobjects = 1 if bool(_tkimpl.wantobjects()) else 0
TkVersion = float(_tkimpl.TK_VERSION)
TclVersion = float(_tkimpl.TCL_VERSION)
READABLE = _tkimpl.READABLE
WRITABLE = _tkimpl.WRITABLE
EXCEPTION = _tkimpl.EXCEPTION


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
    if not isinstance(name, str):
        return name
    return name if name.startswith("-") else f"-{name}"


def _normalize_tk_option_value(owner, value):
    if callable(value):
        if owner is not None and hasattr(owner, "_register"):
            return owner._register(value)
        return value
    if isinstance(value, (list, tuple)):
        normalized_items = []
        changed = False
        for item in value:
            if callable(item):
                if owner is not None and hasattr(owner, "_register"):
                    normalized_items.append(owner._register(item))
                    changed = True
                else:
                    normalized_items.append(item)
            else:
                normalized_items.append(item)
        if changed:
            return type(value)(normalized_items)
    return value


def _normalize_tk_options(cnf=None, *, owner=None, **kw):
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
        normalized.append(_normalize_tk_option_value(owner, value))
    return normalized


def _flatten(seq):
    return _molt_tk_flatten_args(seq)


def _cnfmerge(cnfs):
    return _molt_tk_cnfmerge(cnfs, None)


def _join(value):
    return " ".join(map(_stringify, value))


def _stringify(value):
    if isinstance(value, (list, tuple)):
        if len(value) == 1:
            value = _stringify(value[0])
            if _MAGIC_RE.search(value):
                value = f"{{{value}}}"
        else:
            value = "{" + _join(value) + "}"
    else:
        if isinstance(value, bytes):
            value = str(value, "latin1")
        else:
            value = str(value)
        if not value:
            value = "{}"
        elif _MAGIC_RE.search(value):
            value = _MAGIC_RE.sub(r"\\\1", value)
            value = value.replace("\n", r"\n")
            value = _SPACE_RE.sub(r"\\\1", value)
            if value[0] == '"':
                value = "\\" + value
        elif value[0] == '"' or _SPACE_RE.search(value):
            value = f"{{{value}}}"
    return value


def _splitdict(tk, v, cut_minus=True, conv=None):
    try:
        raw = _molt_tk_splitdict(v, cut_minus)
    except RuntimeError as exc:
        if "intrinsic unavailable: molt_tk_splitdict" not in str(exc):
            raise
        raw = v
    # The intrinsic returns a list of [key, value] pairs; convert to dict.
    if isinstance(raw, dict):
        out = raw
    elif isinstance(raw, (list, tuple)):
        out = {}
        if all(isinstance(pair, (list, tuple)) and len(pair) == 2 for pair in raw):
            for pair in raw:
                out[pair[0]] = pair[1]
        else:
            pairs = list(raw)
            for idx in range(0, len(pairs) - 1, 2):
                out[pairs[idx]] = pairs[idx + 1]
    else:
        out = {}
    if cut_minus:
        out = {
            key[1:] if isinstance(key, str) and key.startswith("-") else key: value
            for key, value in out.items()
        }
    if conv is not None:
        out = {k: conv(val) for k, val in out.items()}
    return out


_VERSION_RE = re.compile(r"(\d+)\.(\d+)([ab.])(\d+)")


class _VersionInfoType:
    __slots__ = ("major", "minor", "micro", "releaselevel", "serial")

    def __init__(self, major, minor, micro, releaselevel, serial):
        self.major = major
        self.minor = minor
        self.micro = micro
        self.releaselevel = releaselevel
        self.serial = serial

    def __str__(self):
        if self.releaselevel == "final":
            return f"{self.major}.{self.minor}.{self.micro}"
        return f"{self.major}.{self.minor}{self.releaselevel[0]}{self.serial}"


def _parse_version(version):
    match = _VERSION_RE.fullmatch(version)
    major, minor, releaselevel, serial = match.groups()
    major, minor, serial = int(major), int(minor), int(serial)
    if releaselevel == ".":
        micro = serial
        serial = 0
        releaselevel = "final"
    else:
        micro = 0
        releaselevel = {"a": "alpha", "b": "beta"}[releaselevel]
    return _VersionInfoType(major, minor, micro, releaselevel, serial)


def _normalize_delay_ms(delay_ms):
    if isinstance(delay_ms, int):
        return delay_ms
    if isinstance(delay_ms, float):
        if delay_ms != delay_ms or delay_ms in (float("inf"), float("-inf")):
            return None
        return int(delay_ms)
    if isinstance(delay_ms, str):
        trimmed = delay_ms.strip()
        if not trimmed:
            return None
        if trimmed.isascii():
            if trimmed.isdigit():
                return int(trimmed)
            if trimmed.startswith("-") and trimmed[1:].isdigit():
                return int(trimmed)
    return None


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


def _resolve_widget_names(owner, names):
    resolved = []
    for name in names:
        try:
            resolved.append(owner.nametowidget(name))
        except Exception:
            resolved.append(name)
    return tuple(resolved)


def _normalize_trace_mode(mode):
    if isinstance(mode, (tuple, list)):
        return " ".join(str(part) for part in mode)
    return str(mode)


def _next_command_name(prefix):
    global _command_serial
    name = f"::__molt_tkinter_{prefix}_{_command_serial}"
    _command_serial += 1
    return name


def _set_default_root(root):
    global _default_root
    if _support_default_root and _default_root is None:
        _default_root = root


def _clear_default_root(root):
    global _default_root
    if _default_root is root:
        _default_root = None


def NoDefaultRoot():
    global _support_default_root, _default_root
    _support_default_root = False
    _default_root = None


def _get_default_root(what=None):
    if not _support_default_root:
        raise RuntimeError(
            "No master specified and tkinter is configured to not support default root"
        )
    if _default_root is None:
        if what:
            raise RuntimeError(f"Too early to {what}: no default root window")
        return Tk()
    return _default_root


def _get_temp_root():
    global _support_default_root
    if not _support_default_root:
        raise RuntimeError(
            "No master specified and tkinter is configured to not support default root"
        )
    root = _default_root
    if root is None:
        _support_default_root = False
        root = Tk()
        _support_default_root = True
        root.withdraw()
        root._temporary = True
    return root


def _destroy_temp_root(master):
    if getattr(master, "_temporary", False):
        destroy = getattr(master, "destroy", None)
        if callable(destroy):
            destroy()


def _tkerror(err):
    del err
    return None


def _exit(code=0):
    if isinstance(code, (int, float)):
        code = int(code)
    elif isinstance(code, str) and code.lstrip("-").isdigit():
        code = int(code)
    raise SystemExit(code)


def _next_variable_name():
    global _variable_serial
    name = f"PY_VAR{_variable_serial}"
    _variable_serial += 1
    return name


def _event_int(widget, value):
    return _molt_tk_event_int(value)


def _event_from_subst_args(widget, event_args):
    widget_path = getattr(widget, "_w", "")

    def _coerce_fallback_event(fields):
        if len(fields) != len(_SUBST_FORMAT):
            return None
        return {
            "serial": _event_int(widget, fields[0]),
            "num": _event_int(widget, fields[1]),
            "focus": bool(_event_int(widget, fields[2])),
            "height": _event_int(widget, fields[3]),
            "keycode": _event_int(widget, fields[4]),
            "state": _event_int(widget, fields[5]),
            "time": _event_int(widget, fields[6]),
            "width": _event_int(widget, fields[7]),
            "x": _event_int(widget, fields[8]),
            "y": _event_int(widget, fields[9]),
            "char": fields[10],
            "send_event": bool(_event_int(widget, fields[11])),
            "keysym": fields[12],
            "keysym_num": _event_int(widget, fields[13]),
            "widget": fields[14],
            "type": fields[15],
            "x_root": _event_int(widget, fields[16]),
            "y_root": _event_int(widget, fields[17]),
            "delta": _event_int(widget, fields[18]),
        }

    result = _molt_tk_event_build_from_args(widget_path, event_args)
    fallback_result = _coerce_fallback_event(event_args)
    if result is None:
        result = fallback_result
        if result is None:
            return None
    if isinstance(result, (list, tuple)) and len(result) == len(_SUBST_FORMAT):
        result = {
            "serial": result[0],
            "num": result[1],
            "focus": result[2],
            "height": result[3],
            "keycode": result[4],
            "state": result[5],
            "time": result[6],
            "width": result[7],
            "x": result[8],
            "y": result[9],
            "char": result[10],
            "send_event": result[11],
            "keysym": result[12],
            "keysym_num": result[13],
            "widget": result[14],
            "type": result[15],
            "x_root": result[16],
            "y_root": result[17],
            "delta": result[18],
        }
    if isinstance(result, list):
        if len(result) == 1 and isinstance(result[0], dict):
            result = result[0]
        elif all(isinstance(item, (list, tuple)) and len(item) == 2 for item in result):
            result = {key: value for key, value in result}
        else:
            result = fallback_result
            if result is None:
                return None
    if isinstance(result, dict) and isinstance(fallback_result, dict):
        merged = dict(fallback_result)
        merged.update(result)
        result = merged
    event = Event()
    for key, value in result.items():
        setattr(event, key, value)
    # Resolve widget reference: if the event's widget path matches
    # this widget, use the widget object instead of the path string.
    evt_widget = getattr(event, "widget", None)
    if isinstance(evt_widget, str) and evt_widget:
        event.widget = widget if evt_widget == widget_path else evt_widget
    else:
        event.widget = widget
    return event


class Event:
    """Minimal tkinter event object placeholder for bind callbacks."""

    def __repr__(self):
        attrs = {key: value for key, value in self.__dict__.items() if value != "??"}

        char_value = attrs.get("char")
        if not char_value:
            attrs.pop("char", None)
        elif char_value != "??":
            attrs["char"] = repr(char_value)

        if not getattr(self, "send_event", True):
            attrs.pop("send_event", None)

        state_value = attrs.get("state")
        if state_value == 0:
            attrs.pop("state", None)
        elif isinstance(state_value, int):
            parts = _molt_tk_event_state_decode(state_value)
            attrs["state"] = "|".join(parts)

        if attrs.get("delta") == 0:
            attrs.pop("delta", None)

        keys = (
            "send_event",
            "state",
            "keysym",
            "keycode",
            "char",
            "num",
            "delta",
            "focus",
            "x",
            "y",
            "width",
            "height",
        )
        event_type = getattr(self, "type", "?")
        event_type_name = getattr(event_type, "name", event_type)
        return "<%s event%s>" % (
            event_type_name,
            "".join(f" {key}={attrs[key]}" for key in keys if key in attrs),
        )


class EventType(str, _EventTypeBase):
    KeyPress = "2"
    Key = KeyPress
    KeyRelease = "3"
    ButtonPress = "4"
    Button = ButtonPress
    ButtonRelease = "5"
    Motion = "6"
    Enter = "7"
    Leave = "8"
    FocusIn = "9"
    FocusOut = "10"
    Keymap = "11"
    Expose = "12"
    GraphicsExpose = "13"
    NoExpose = "14"
    Visibility = "15"
    Create = "16"
    Destroy = "17"
    Unmap = "18"
    Map = "19"
    MapRequest = "20"
    Reparent = "21"
    Configure = "22"
    ConfigureRequest = "23"
    Gravity = "24"
    ResizeRequest = "25"
    Circulate = "26"
    CirculateRequest = "27"
    Property = "28"
    SelectionClear = "29"
    SelectionRequest = "30"
    Selection = "31"
    Colormap = "32"
    ClientMessage = "33"
    Mapping = "34"
    VirtualEvent = "35"
    Activate = "36"
    Deactivate = "37"
    MouseWheel = "38"


class Misc:
    """Shared object helpers for Tk and widgets."""

    _last_child_ids = None
    _tclCommands = None
    _subst_format = _SUBST_FORMAT
    _subst_format_str = _SUBST_FORMAT_STR
    _noarg_ = NO_VALUE

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
        deletecommand = getattr(self, "deletecommand", None)
        if callable(deletecommand):
            deletecommand(name)
        else:
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
        return self._call_widget(
            "configure", *_normalize_tk_options(cnf, owner=self, **kw)
        )

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

    def __str__(self):
        return self._w

    def __repr__(self):
        return "<%s.%s object %s>" % (
            self.__class__.__module__,
            self.__class__.__qualname__,
            self._w,
        )

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
        if args:

            def wrapped():
                return callback(*args)

            return _tkimpl.after_idle(self._tk_app, wrapped)
        return _tkimpl.after_idle(self._tk_app, callback)

    def after_cancel(self, identifier):
        _require_gui_window_capability()
        if not identifier:
            raise ValueError(
                "id must be a valid identifier returned from after or after_idle"
            )

        delete_timer = getattr(identifier, "deletetimerhandler", None)
        if callable(delete_timer):
            delete_timer()
            return None

        token = getattr(identifier, "_token", identifier)
        _tkimpl.after_cancel(self._tk_app, token)
        return None

    def after_info(self, identifier=None):
        _require_gui_window_capability()
        return _tkimpl.after_info(self._tk_app, identifier)

    def bind_command(self, name, callback):
        _require_gui_window_capability()
        if not callable(callback):
            raise TypeError("bind_command callback must be callable")
        _TK_BIND_COMMAND(self._tk_app, name, callback)

    def createcommand(self, name, callback):
        _require_gui_window_capability()
        _tkimpl.createcommand(self._tk_app, name, callback)
        root = getattr(self, "tk", None)
        if root is not None and hasattr(root, "_registered_commands"):
            root._registered_commands.add(str(name))

    def deletecommand(self, name):
        value = _tkimpl.deletecommand(self._tk_app, name)
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
            parsed_event = _event_from_subst_args(widget, event_args)
            if parsed_event is not None:
                return func(parsed_event)
            event = Event()
            event.widget = widget
            if event_args:
                event.args = event_args
            return func(event)

        command_name = _TK_BIND_REGISTER(
            self._tk_app,
            target_name,
            sequence,
            wrapped,
            add_prefix,
        )
        root = getattr(self, "tk", None)
        if root is not None and hasattr(root, "_registered_commands"):
            root._registered_commands.add(str(command_name))
        return command_name

    def _unbind(self, target, sequence, funcid=None):
        _require_gui_window_capability()
        if sequence is None:
            raise TypeError("unbind sequence must not be None")

        target_name = _normalize_bind_target(target)
        if funcid is None:
            return self.call("bind", target_name, sequence, "")

        command_name = str(funcid)
        _TK_BIND_UNREGISTER(self._tk_app, target_name, sequence, command_name)
        root = getattr(self, "tk", None)
        if root is not None and hasattr(root, "_registered_commands"):
            root._registered_commands.discard(command_name)
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
        self.call("destroy", self._w)

    def last_error(self):
        _require_gui_window_capability()
        return _TK_LAST_ERROR(self._tk_app)

    def getboolean(self, value):
        return _tkimpl.getboolean(value)

    def getint(self, value):
        return _tkimpl.getint(value)

    def getdouble(self, value):
        return _tkimpl.getdouble(value)

    def splitlist(self, value):
        return _tkimpl.splitlist(value)

    def getvar(self, name="PY_VAR"):
        return _tkimpl.getvar(self._tk_app, name)

    def setvar(self, name="PY_VAR", value="1"):
        return _tkimpl.setvar(self._tk_app, name, value)

    def unsetvar(self, name="PY_VAR"):
        return _tkimpl.unsetvar(self._tk_app, name)

    def globalgetvar(self, name="PY_VAR"):
        return _tkimpl.globalgetvar(self._tk_app, name)

    def globalsetvar(self, name="PY_VAR", value="1"):
        return _tkimpl.globalsetvar(self._tk_app, name, value)

    def globalunsetvar(self, name="PY_VAR"):
        return _tkimpl.globalunsetvar(self._tk_app, name)

    def wait_variable(self, name="PY_VAR"):
        _require_gui_window_capability()
        variable_name = name._name if hasattr(name, "_name") else name
        return _TK_WAIT_VARIABLE(self._tk_app, variable_name)

    waitvar = wait_variable

    def wait_window(self, window=None):
        _require_gui_window_capability()
        target = self._w if window is None else _normalize_bind_target(window)
        return _TK_WAIT_WINDOW(self._tk_app, target)

    def wait_visibility(self, window=None):
        _require_gui_window_capability()
        target = self._w if window is None else _normalize_bind_target(window)
        return _TK_WAIT_VISIBILITY(self._tk_app, target)

    def tk_strictMotif(self, boolean=None):
        if boolean is None:
            return self.getboolean(self.call("set", "tk_strictMotif"))
        return self.call("set", "tk_strictMotif", int(bool(boolean)))

    def tk_bisque(self):
        return self.call("tk_bisque")

    def tk_setPalette(self, *args, **kw):
        if args and kw:
            raise TypeError(
                "tk_setPalette() cannot mix positional arguments and kwargs"
            )
        if kw:
            return self.call("tk_setPalette", *_normalize_tk_options(None, **kw))
        return self.call("tk_setPalette", *args)

    if sys.version_info >= (3, 13):

        def tk_busy_hold(self, **kw):
            """Indicate that the widget is busy."""
            args = _normalize_tk_options(None, **kw) if kw else []
            self.call("tk", "busy", "hold", self._w, *args)

        tk_busy = tk_busy_hold

        def tk_busy_configure(self, cnf=None, **kw):
            """Query or modify busy options."""
            if cnf is not None and not isinstance(cnf, dict):
                if kw:
                    raise TypeError(
                        "tk_busy_configure() option query cannot be combined with "
                        "update options"
                    )
                return self.call(
                    "tk", "busy", "configure", self._w, _normalize_option_name(cnf)
                )
            if cnf is None and not kw:
                return self.call("tk", "busy", "configure", self._w)
            return self.call(
                "tk",
                "busy",
                "configure",
                self._w,
                *_normalize_tk_options(cnf, **kw),
            )

        def tk_busy_cget(self, option):
            """Return the value of a busy option."""
            return self.call(
                "tk", "busy", "cget", self._w, _normalize_option_name(option)
            )

        def tk_busy_forget(self):
            """Release the busy hold."""
            self.call("tk", "busy", "forget", self._w)

        def tk_busy_current(self, pattern=None):
            """Return list of widgets with busy hold."""
            if pattern is not None:
                return self.splitlist(self.call("tk", "busy", "current", pattern))
            return self.splitlist(self.call("tk", "busy", "current"))

        def tk_busy_status(self):
            """Return True if widget has a busy hold."""
            return self.getboolean(self.call("tk", "busy", "status", self._w))

    def focus_set(self):
        return self.call("focus", self._w)

    def focus_force(self):
        return self.call("focus", "-force", self._w)

    def focus_get(self):
        return self.call("focus")

    def focus_displayof(self):
        return self.call("focus", "-displayof", self._w)

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

    def option_add(self, pattern, value, priority=None):
        if priority is None:
            return self.call("option", "add", pattern, value)
        return self.call("option", "add", pattern, value, priority)

    def option_clear(self):
        return self.call("option", "clear")

    def option_get(self, name, class_name):
        return self.call("option", "get", self._w, name, class_name)

    def option_readfile(self, file_name, priority=None):
        if priority is None:
            return self.call("option", "readfile", file_name)
        return self.call("option", "readfile", file_name, priority)

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

    def selection_handle(self, command, **kw):
        if isinstance(command, str):
            callback_name = command
            should_release = False
        elif callable(command):
            callback_name = self._register_command("selection_handle", command)
            should_release = True
        else:
            raise TypeError(
                "selection_handle command must be a callback or command name"
            )
        try:
            return self.call(
                "selection",
                "handle",
                callback_name,
                self._w,
                *_normalize_tk_options(None, **kw),
            )
        except Exception:
            if should_release:
                self._release_command(callback_name)
            raise

    def selection_own(self, **kw):
        if not kw:
            return self.call("selection", "own", self._w)
        return self.call(
            "selection", "own", self._w, *_normalize_tk_options(None, **kw)
        )

    def selection_own_get(self, **kw):
        return self.call("selection", "own", *_normalize_tk_options(None, **kw))

    def send(self, interp, cmd, *args):
        return self.call("send", interp, cmd, *args)

    def pack_configure(self, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "pack_configure() option query cannot be combined with updates"
                )
            return self.call("pack", "configure", self._w, _normalize_option_name(cnf))
        return self.call(
            "pack", "configure", self._w, *_normalize_tk_options(cnf, owner=self, **kw)
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
        return _resolve_widget_names(
            self, self.splitlist(self.call("pack", "slaves", self._w))
        )

    pack_children = pack_slaves

    def grid_configure(self, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "grid_configure() option query cannot be combined with updates"
                )
            return self.call("grid", "configure", self._w, _normalize_option_name(cnf))
        return self.call(
            "grid", "configure", self._w, *_normalize_tk_options(cnf, owner=self, **kw)
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

    def grid_anchor(self, anchor=None):
        if anchor is None:
            return self.call("grid", "anchor", self._w)
        return self.call("grid", "anchor", self._w, anchor)

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
            *_normalize_tk_options(cnf, owner=self, **kw),
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
            *_normalize_tk_options(cnf, owner=self, **kw),
        )

    columnconfigure = grid_columnconfigure
    rowconfigure = grid_rowconfigure

    def grid_slaves(self, row=None, column=None):
        args = [self._w]
        if row is not None:
            args.extend(("-row", row))
        if column is not None:
            args.extend(("-column", column))
        return _resolve_widget_names(
            self, self.splitlist(self.call("grid", "slaves", *args))
        )

    grid_children = grid_slaves

    def place_configure(self, cnf=None, **kw):
        if isinstance(cnf, str):
            if kw:
                raise TypeError(
                    "place_configure() option query cannot be combined with updates"
                )
            return self.call("place", "configure", self._w, _normalize_option_name(cnf))
        return self.call(
            "place", "configure", self._w, *_normalize_tk_options(cnf, owner=self, **kw)
        )

    place = place_configure

    def place_forget(self):
        return self.call("place", "forget", self._w)

    def place_info(self):
        return self.call("place", "info", self._w)

    def place_slaves(self):
        return _resolve_widget_names(
            self, self.splitlist(self.call("place", "slaves", self._w))
        )

    place_children = place_slaves

    def lift(self, above_this=None):
        if above_this is None:
            return self.call("raise", self._w)
        return self.call("raise", self._w, _normalize_bind_target(above_this))

    def tkraise(self, above_this=None):
        return self.lift(above_this)

    def lower(self, below_this=None):
        if below_this is None:
            return self.call("lower", self._w)
        return self.call("lower", self._w, _normalize_bind_target(below_this))

    def winfo_children(self):
        local_children = tuple(getattr(self, "children", {}).values())
        if local_children:
            return local_children
        return _resolve_widget_names(
            self, self.splitlist(self.call("winfo", "children", self._w))
        )

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

    def info_patchlevel(self):
        patchlevel = self.call("info", "patchlevel")
        return _parse_version(patchlevel)

    def winfo_cells(self):
        return self.getint(self.call("winfo", "cells", self._w))

    def winfo_colormapfull(self):
        return bool(self.getint(self.call("winfo", "colormapfull", self._w)))

    def winfo_depth(self):
        return self.getint(self.call("winfo", "depth", self._w))

    def winfo_fpixels(self, number):
        return self.getdouble(self.call("winfo", "fpixels", self._w, number))

    def winfo_geometry(self):
        return self.call("winfo", "geometry", self._w)

    def winfo_interps(self, displayof=None):
        if displayof is None:
            return self.splitlist(self.call("winfo", "interps"))
        return self.splitlist(
            self.call(
                "winfo", "interps", "-displayof", _normalize_bind_target(displayof)
            )
        )

    def winfo_pathname(self, window_id, displayof=None):
        if displayof is None:
            return self.call("winfo", "pathname", window_id)
        return self.call(
            "winfo",
            "pathname",
            "-displayof",
            _normalize_bind_target(displayof),
            window_id,
        )

    def winfo_pixels(self, number):
        return self.getint(self.call("winfo", "pixels", self._w, number))

    def winfo_screen(self):
        return self.call("winfo", "screen", self._w)

    def winfo_screencells(self):
        return self.getint(self.call("winfo", "screencells", self._w))

    def winfo_screendepth(self):
        return self.getint(self.call("winfo", "screendepth", self._w))

    def winfo_screenmmheight(self):
        return self.getint(self.call("winfo", "screenmmheight", self._w))

    def winfo_screenmmwidth(self):
        return self.getint(self.call("winfo", "screenmmwidth", self._w))

    def winfo_screenvisual(self):
        return self.call("winfo", "screenvisual", self._w)

    def winfo_server(self):
        return self.call("winfo", "server", self._w)

    def winfo_visual(self):
        return self.call("winfo", "visual", self._w)

    def winfo_visualid(self):
        return self.call("winfo", "visualid", self._w)

    def winfo_visualsavailable(self, includeids=False):
        values = self.splitlist(
            self.call("winfo", "visualsavailable", self._w, int(bool(includeids)))
        )
        if not includeids:
            return values
        parsed = []
        for value in values:
            entry = self.splitlist(value)
            if len(entry) >= 3:
                parsed.append((entry[0], self.getint(entry[1]), entry[2]))
            elif len(entry) == 2:
                parsed.append((entry[0], self.getint(entry[1])))
            elif entry:
                parsed.append(tuple(entry))
        return parsed

    def winfo_vrootheight(self):
        return self.getint(self.call("winfo", "vrootheight", self._w))

    def winfo_vrootwidth(self):
        return self.getint(self.call("winfo", "vrootwidth", self._w))

    def winfo_vrootx(self):
        return self.getint(self.call("winfo", "vrootx", self._w))

    def winfo_vrooty(self):
        return self.getint(self.call("winfo", "vrooty", self._w))

    def _iter_widget_tree(self):
        root = self.tk if hasattr(self, "tk") else self
        stack = [root]
        seen = set()
        while stack:
            widget = stack.pop()
            ident = id(widget)
            if ident in seen:
                continue
            seen.add(ident)
            yield widget
            for child in getattr(widget, "children", {}).values():
                stack.append(child)

    def nametowidget(self, name):
        if isinstance(name, Misc):
            return name
        widget_name = str(name)
        if widget_name in ("", "."):
            return self.tk if hasattr(self, "tk") else self
        if not widget_name.startswith("."):
            prefix = getattr(self, "_w", ".")
            if prefix == ".":
                widget_name = f".{widget_name}"
            else:
                widget_name = f"{prefix}.{widget_name}"

        for widget in self._iter_widget_tree():
            if getattr(widget, "_w", None) == widget_name:
                return widget
        raise KeyError(f"unknown widget '{widget_name}'")

    _nametowidget = nametowidget

    def _register(self, func, subst=None, needcleanup=1):
        del needcleanup
        callback = CallWrapper(func, subst, self) if subst else func
        return self._register_command("register", callback)

    def image_names(self):
        return self.splitlist(self.call("image", "names"))

    def image_types(self):
        return self.splitlist(self.call("image", "types"))

    def _root(self):
        """Return the root Toplevel (Tk) widget."""
        root = getattr(self, "tk", self)
        return root

    focus = focus_set
    register = _register
    propagate = pack_propagate
    slaves = pack_slaves
    anchor = grid_anchor
    bbox = grid_bbox
    size = grid_size


class CallWrapper:
    """Compatibility callback wrapper used by Tk command bridges."""

    def __init__(self, func, subst, widget):
        self.func = func
        self.subst = subst
        self.widget = widget

    def __call__(self, *args):
        if self.subst:
            args = self.subst(*args)
        return self.func(*args)


class XView:
    def xview(self, *args):
        result = self._call_widget("xview", *args)
        if args:
            return result
        return tuple(self.getdouble(part) for part in self.splitlist(result))

    def xview_moveto(self, fraction):
        return self._call_widget("xview", "moveto", fraction)

    def xview_scroll(self, number, what):
        return self._call_widget("xview", "scroll", number, what)


class YView:
    def yview(self, *args):
        result = self._call_widget("yview", *args)
        if args:
            return result
        return tuple(self.getdouble(part) for part in self.splitlist(result))

    def yview_moveto(self, fraction):
        return self._call_widget("yview", "moveto", fraction)

    def yview_scroll(self, number, what):
        return self._call_widget("yview", "scroll", number, what)


class Pack:
    pack = configure = config = Misc.pack_configure
    forget = Misc.pack_forget
    info = Misc.pack_info
    propagate = pack_propagate = Misc.pack_propagate
    slaves = pack_slaves = Misc.pack_slaves

    def pack_configure(self, cnf=None, **kw):
        return Misc.pack_configure(self, cnf, **kw)

    def pack_forget(self):
        return Misc.pack_forget(self)

    def pack_info(self):
        return Misc.pack_info(self)


class Place:
    place = configure = config = Misc.place_configure
    forget = Misc.place_forget
    info = Misc.place_info
    slaves = place_slaves = Misc.place_slaves

    def place_configure(self, cnf=None, **kw):
        return Misc.place_configure(self, cnf, **kw)

    def place_forget(self):
        return Misc.place_forget(self)

    def place_info(self):
        return Misc.place_info(self)


class Grid:
    grid = configure = config = Misc.grid_configure
    bbox = grid_bbox = Misc.grid_bbox
    columnconfigure = grid_columnconfigure = Misc.grid_columnconfigure
    forget = Misc.grid_forget
    info = Misc.grid_info
    location = grid_location = Misc.grid_location
    propagate = grid_propagate = Misc.grid_propagate
    rowconfigure = grid_rowconfigure = Misc.grid_rowconfigure
    size = grid_size = Misc.grid_size
    slaves = grid_slaves = Misc.grid_slaves

    def grid_configure(self, cnf=None, **kw):
        return Misc.grid_configure(self, cnf, **kw)

    def grid_forget(self):
        return Misc.grid_forget(self)

    def grid_remove(self):
        return Misc.grid_remove(self)

    def grid_info(self):
        return Misc.grid_info(self)


class Wm(Misc):
    """Window-manager mixin for Tk and toplevel-style widgets."""

    def _wm_call(self, command, *args):
        return self.call("wm", command, self._w, *args)

    def wm_aspect(self, min_num=None, min_denom=None, max_num=None, max_denom=None):
        if (
            min_num is None
            and min_denom is None
            and max_num is None
            and max_denom is None
        ):
            result = self._wm_call("aspect")
            if not result:
                return None
            return self._split_ints(result)
        if None in (min_num, min_denom, max_num, max_denom):
            raise TypeError(
                "wm_aspect() requires min_num, min_denom, max_num, max_denom"
            )
        return self._wm_call("aspect", min_num, min_denom, max_num, max_denom)

    def wm_attributes(self, *args):
        if not args:
            return self._wm_call("attributes")
        if len(args) == 1 and isinstance(args[0], dict):
            return self._wm_call("attributes", *_normalize_tk_options(args[0]))
        return self._wm_call("attributes", *args)

    def wm_client(self, name=None):
        if name is None:
            return self._wm_call("client")
        return self._wm_call("client", name)

    def wm_colormapwindows(self, *wlist):
        if not wlist:
            return self.splitlist(self._wm_call("colormapwindows"))
        windows = tuple(_normalize_bind_target(widget) for widget in wlist)
        return self._wm_call("colormapwindows", windows)

    def wm_command(self, value=None):
        if value is None:
            return self._wm_call("command")
        if isinstance(value, (tuple, list)):
            value = tuple(value)
        return self._wm_call("command", value)

    def wm_deiconify(self):
        return self._wm_call("deiconify")

    def wm_focusmodel(self, model=None):
        if model is None:
            return self._wm_call("focusmodel")
        return self._wm_call("focusmodel", model)

    def wm_forget(self, window):
        return self.call("wm", "forget", _normalize_bind_target(window))

    def wm_frame(self):
        return self._wm_call("frame")

    def wm_geometry(self, new_geometry=None):
        if new_geometry is None:
            return self._wm_call("geometry")
        return self._wm_call("geometry", new_geometry)

    def wm_grid(
        self, base_width=None, base_height=None, width_inc=None, height_inc=None
    ):
        if (
            base_width is None
            and base_height is None
            and width_inc is None
            and height_inc is None
        ):
            result = self._wm_call("grid")
            if not result:
                return None
            return self._split_ints(result)
        if None in (base_width, base_height, width_inc, height_inc):
            raise TypeError(
                "wm_grid() requires base_width, base_height, width_inc, and height_inc"
            )
        return self._wm_call("grid", base_width, base_height, width_inc, height_inc)

    def wm_group(self, path_name=None):
        if path_name is None:
            return self._wm_call("group")
        return self._wm_call("group", _normalize_bind_target(path_name))

    def wm_iconbitmap(self, bitmap=None, default=None):
        if bitmap is None and default is None:
            return self._wm_call("iconbitmap")
        args = []
        if default is not None:
            args.extend(("-default", default))
        if bitmap is not None:
            args.append(bitmap)
        return self._wm_call("iconbitmap", *args)

    def wm_iconify(self):
        return self._wm_call("iconify")

    def wm_iconmask(self, bitmap=None):
        if bitmap is None:
            return self._wm_call("iconmask")
        return self._wm_call("iconmask", bitmap)

    def wm_iconname(self, new_name=None):
        if new_name is None:
            return self._wm_call("iconname")
        return self._wm_call("iconname", new_name)

    def wm_iconphoto(self, default=False, *args):
        photo_args = []
        if default:
            photo_args.append("-default")
        photo_args.extend(str(photo) for photo in args)
        return self._wm_call("iconphoto", *photo_args)

    def wm_iconposition(self, x=None, y=None):
        if x is None and y is None:
            result = self._wm_call("iconposition")
            if not result:
                return None
            return self._split_ints(result)
        if x is None or y is None:
            raise TypeError("wm_iconposition() requires both x and y")
        return self._wm_call("iconposition", x, y)

    def wm_iconwindow(self, path_name=None):
        if path_name is None:
            return self._wm_call("iconwindow")
        return self._wm_call("iconwindow", _normalize_bind_target(path_name))

    def wm_manage(self, widget):
        return self.call("wm", "manage", _normalize_bind_target(widget))

    def wm_maxsize(self, width=None, height=None):
        if width is None and height is None:
            return self._split_ints(self._wm_call("maxsize"))
        if width is None or height is None:
            raise TypeError("wm_maxsize() requires both width and height")
        return self._wm_call("maxsize", width, height)

    def wm_minsize(self, width=None, height=None):
        if width is None and height is None:
            return self._split_ints(self._wm_call("minsize"))
        if width is None or height is None:
            raise TypeError("wm_minsize() requires both width and height")
        return self._wm_call("minsize", width, height)

    def wm_overrideredirect(self, boolean=NO_VALUE):
        if boolean is NO_VALUE:
            return self.getboolean(self._wm_call("overrideredirect"))
        return self._wm_call("overrideredirect", int(bool(boolean)))

    def wm_positionfrom(self, who=None):
        if who is None:
            return self._wm_call("positionfrom")
        return self._wm_call("positionfrom", who)

    def wm_protocol(self, name=None, func=None):
        if name is None:
            return self._wm_call("protocol")

        commands = (
            self._protocol_commands
            if hasattr(self, "_protocol_commands")
            and isinstance(self._protocol_commands, dict)
            else None
        )
        if commands is not None:
            previous = commands.pop(name, None)
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
        if commands is not None:
            commands[name] = command_name
        return command_name

    def wm_resizable(self, width=None, height=None):
        if width is None and height is None:
            values = self.splitlist(self._wm_call("resizable"))
            if len(values) >= 2:
                return (self.getboolean(values[0]), self.getboolean(values[1]))
            return tuple(self.getboolean(value) for value in values)
        if width is None or height is None:
            raise TypeError("wm_resizable() requires both width and height")
        return self._wm_call("resizable", int(bool(width)), int(bool(height)))

    def wm_sizefrom(self, who=None):
        if who is None:
            return self._wm_call("sizefrom")
        return self._wm_call("sizefrom", who)

    def wm_state(self, new_state=None):
        if new_state is None:
            return self._wm_call("state")
        return self._wm_call("state", new_state)

    def wm_title(self, string=None):
        if string is None:
            return self._wm_call("title")
        return self._wm_call("title", string)

    def wm_transient(self, master=None):
        if master is None:
            return self._wm_call("transient")
        return self._wm_call("transient", _normalize_bind_target(master))

    def wm_withdraw(self):
        return self._wm_call("withdraw")

    aspect = wm_aspect
    attributes = wm_attributes
    client = wm_client
    colormapwindows = wm_colormapwindows
    command = wm_command
    deiconify = wm_deiconify
    focusmodel = wm_focusmodel
    forget = wm_forget
    frame = wm_frame
    geometry = wm_geometry
    grid = wm_grid
    group = wm_group
    iconbitmap = wm_iconbitmap
    iconify = wm_iconify
    iconmask = wm_iconmask
    iconname = wm_iconname
    iconphoto = wm_iconphoto
    iconposition = wm_iconposition
    iconwindow = wm_iconwindow
    manage = wm_manage
    maxsize = wm_maxsize
    minsize = wm_minsize
    overrideredirect = wm_overrideredirect
    positionfrom = wm_positionfrom
    protocol = wm_protocol
    resizable = wm_resizable
    sizefrom = wm_sizefrom
    state = wm_state
    title = wm_title
    transient = wm_transient
    withdraw = wm_withdraw


def Tcl(screenName=None, baseName=None, className="Tk", useTk=False):
    return Tk(screenName, baseName, className, useTk)


class Tk(Wm):
    """Root window wrapper backed by `_tkinter` intrinsics."""

    _w = "."

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
        self._tkloaded = False
        self._tk_app = _TK_CREATE(options=options)
        self._registered_commands = set()
        self._protocol_commands = {}
        self.children = {}
        self._w = "."
        self.tk = self
        self._widget_serial = 0
        if useTk:
            self._loadtk()
        _set_default_root(self)

    def loadtk(self):
        if not self._tkloaded:
            self._tk_app.loadtk()
            self._loadtk()
        return None

    def _loadtk(self):
        self._tkloaded = True
        self._windowingsystem = self.call("tk", "windowingsystem")
        self.createcommand("tkerror", _tkerror)
        self.createcommand("exit", _exit)
        self.protocol("WM_DELETE_WINDOW", self.destroy)

    def readprofile(self, baseName, className):
        del baseName, className
        return None

    def report_callback_exception(self, exc, val, tb):
        import traceback as _traceback

        _traceback.print_exception(exc, val, tb, file=sys.stderr)

    def _next_widget_path(self, parent_path, widget_command):
        base = widget_command.replace("::", "_").replace("-", "_")
        if not base:
            base = "widget"
        self._widget_serial += 1
        child_name = f"!{base}{self._widget_serial}"
        if parent_path in ("", "."):
            return child_name, f".{child_name}"
        return child_name, f"{parent_path}.{child_name}"

    def _wm_call(self, command, *args):
        return self.call("wm", command, self._w, *args)

    def _purge_registered_commands(self):
        deletecommand = getattr(_tkimpl, "deletecommand", None)
        if callable(deletecommand):
            for command_name in list(self._registered_commands):
                deletecommand(self._tk_app, command_name)
        self._registered_commands.clear()
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

    def __getattr__(self, attr):
        try:
            tk_app = object.__getattribute__(self, "_tk_app")
        except AttributeError as exc:
            raise AttributeError(attr) from exc
        return getattr(tk_app, attr)


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
            "peer", "create", new_path_name, *_normalize_tk_options(cnf, owner=self, **kw)
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


def after_info(identifier=None):
    return _get_default_root().after_info(identifier)


def bind_register(app, target_name, sequence, callback, add_prefix=""):
    return _TK_BIND_REGISTER(app, target_name, sequence, callback, add_prefix)


def bind_unregister(app, target_name, sequence, command_name):
    return _TK_BIND_UNREGISTER(app, target_name, sequence, command_name)


def treeview_tag_bind_register(app, treeview_path, tagname, sequence, callback):
    register = _require_tk_callable("treeview_tag_bind_register")
    return register(app, treeview_path, tagname, sequence, callback)


def treeview_tag_bind_unregister(app, treeview_path, tagname, sequence, command_name):
    unregister = _require_tk_callable("treeview_tag_bind_unregister")
    return unregister(app, treeview_path, tagname, sequence, command_name)


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
    return _tkimpl.getboolean(value)


def getint(value):
    return _tkimpl.getint(value)


def getdouble(value):
    return _tkimpl.getdouble(value)


def splitlist(value):
    return _tkimpl.splitlist(value)


def _print_command(cmd, *, file=sys.stderr):
    if not isinstance(cmd, tuple):
        cmd = tuple(cmd)
    print(_join(cmd), file=file)


def _test():
    root = Tk()
    text = f"This is Tcl/Tk {root.globalgetvar('tk_patchLevel')}"
    text += "\nThis should be a cedilla: \xe7"
    label = Label(root, text=text)
    label.pack()
    test_button = Button(
        root,
        text="Click me!",
        command=lambda root=root: root.test.configure(text=f"[{root.test['text']}]"),
    )
    test_button.pack()
    root.test = test_button
    quit_button = Button(root, text="QUIT", command=root.destroy)
    quit_button.pack()
    root.iconify()
    root.update()
    root.deiconify()
    root.mainloop()


__all__ = [
    "BaseWidget",
    "BitmapImage",
    "BooleanVar",
    "Button",
    "CallWrapper",
    "Canvas",
    "Checkbutton",
    "DoubleVar",
    "Entry",
    "Event",
    "EventType",
    "Frame",
    "Grid",
    "Image",
    "IntVar",
    "Label",
    "LabelFrame",
    "Listbox",
    "Menu",
    "Menubutton",
    "Message",
    "Misc",
    "NoDefaultRoot",
    "OptionMenu",
    "Pack",
    "PanedWindow",
    "PhotoImage",
    "Place",
    "Radiobutton",
    "READABLE",
    "Scale",
    "Scrollbar",
    "Spinbox",
    "StringVar",
    "Tcl",
    "TclError",
    "TclVersion",
    "Text",
    "Tk",
    "TkVersion",
    "Toplevel",
    "Variable",
    "Widget",
    "Wm",
    "WRITABLE",
    "XView",
    "YView",
    "EXCEPTION",
    "after",
    "after_cancel",
    "after_info",
    "after_idle",
    "bind_register",
    "bind_unregister",
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
    "treeview_tag_bind_register",
    "treeview_tag_bind_unregister",
    "unsetvar",
    "image_names",
    "image_types",
    "wantobjects",
]

for _name in tuple(globals()):
    if _name.isupper() and not _name.startswith("_") and _name not in __all__:
        __all__.append(_name)


def __getattr__(attr):
    raise AttributeError(f'module "{__name__}" has no attribute "{attr}"')
