"""Intrinsic-backed `_tkinter` compatibility surface.

This module intentionally keeps behavior minimal while exposing a broad
CPython-shaped API. Runtime behavior is delegated to Molt Rust intrinsics.
"""

import sys

from _intrinsics import require_intrinsic as _require_intrinsic


def _lazy_intrinsic(name):
    def _call(*args, **kwargs):
        return _require_intrinsic(name, globals())(*args, **kwargs)

    return _call


_MOLT_CAPABILITIES_HAS = _lazy_intrinsic("molt_capabilities_has")
_MOLT_TK_AVAILABLE = _lazy_intrinsic("molt_tk_available")
_MOLT_TK_APP_NEW = _lazy_intrinsic("molt_tk_app_new")
_MOLT_TK_QUIT = _lazy_intrinsic("molt_tk_quit")
_MOLT_TK_MAINLOOP = _lazy_intrinsic("molt_tk_mainloop")
_MOLT_TK_DO_ONE_EVENT = _lazy_intrinsic("molt_tk_do_one_event")
_MOLT_TK_AFTER = _lazy_intrinsic("molt_tk_after")
_MOLT_TK_AFTER_IDLE = _lazy_intrinsic("molt_tk_after_idle")
_MOLT_TK_AFTER_CANCEL = _lazy_intrinsic("molt_tk_after_cancel")
_MOLT_TK_AFTER_INFO = _lazy_intrinsic("molt_tk_after_info")
_MOLT_TK_CALL = _require_intrinsic("molt_tk_call", globals())
_MOLT_TK_TRACE_ADD = _lazy_intrinsic("molt_tk_trace_add")
_MOLT_TK_TRACE_REMOVE = _lazy_intrinsic("molt_tk_trace_remove")
_MOLT_TK_TRACE_CLEAR = _lazy_intrinsic("molt_tk_trace_clear")
_MOLT_TK_TRACE_INFO = _lazy_intrinsic("molt_tk_trace_info")
_MOLT_TK_TKWAIT_VARIABLE = _lazy_intrinsic("molt_tk_tkwait_variable")
_MOLT_TK_TKWAIT_WINDOW = _lazy_intrinsic("molt_tk_tkwait_window")
_MOLT_TK_TKWAIT_VISIBILITY = _lazy_intrinsic("molt_tk_tkwait_visibility")
_MOLT_TK_BIND_CALLBACK_REGISTER = _lazy_intrinsic("molt_tk_bind_callback_register")
_MOLT_TK_BIND_CALLBACK_UNREGISTER = _lazy_intrinsic(
    "molt_tk_bind_callback_unregister"
)
_MOLT_TK_WIDGET_BIND_CALLBACK_REGISTER = _lazy_intrinsic(
    "molt_tk_widget_bind_callback_register"
)
_MOLT_TK_WIDGET_BIND_CALLBACK_UNREGISTER = _lazy_intrinsic(
    "molt_tk_widget_bind_callback_unregister"
)
_MOLT_TK_TEXT_TAG_BIND_CALLBACK_REGISTER = _lazy_intrinsic(
    "molt_tk_text_tag_bind_callback_register"
)
_MOLT_TK_TEXT_TAG_BIND_CALLBACK_UNREGISTER = _lazy_intrinsic(
    "molt_tk_text_tag_bind_callback_unregister"
)
_MOLT_TK_TREEVIEW_TAG_BIND_CALLBACK_REGISTER = _lazy_intrinsic(
    "molt_tk_treeview_tag_bind_callback_register"
)
_MOLT_TK_TREEVIEW_TAG_BIND_CALLBACK_UNREGISTER = _lazy_intrinsic(
    "molt_tk_treeview_tag_bind_callback_unregister"
)
_MOLT_TK_BIND_COMMAND = _lazy_intrinsic("molt_tk_bind_command")
_MOLT_TK_UNBIND_COMMAND = _lazy_intrinsic("molt_tk_unbind_command")
_MOLT_TK_FILEHANDLER_CREATE = _lazy_intrinsic("molt_tk_filehandler_create")
_MOLT_TK_FILEHANDLER_DELETE = _lazy_intrinsic("molt_tk_filehandler_delete")
_MOLT_TK_DESTROY_WIDGET = _lazy_intrinsic("molt_tk_destroy_widget")
_MOLT_TK_LAST_ERROR = _lazy_intrinsic("molt_tk_last_error")
_MOLT_TK_GETBOOLEAN = _lazy_intrinsic("molt_tk_getboolean")
_MOLT_TK_GETDOUBLE = _lazy_intrinsic("molt_tk_getdouble")
_MOLT_TK_SPLITLIST = _lazy_intrinsic("molt_tk_splitlist")
_MOLT_TK_ERRORINFO_APPEND = _lazy_intrinsic("molt_tk_errorinfo_append")
_MOLT_TK_BIND_SCRIPT_REMOVE_COMMAND = _lazy_intrinsic(
    "molt_tk_bind_script_remove_command"
)

# CPython exports these constants from `_tkinter`; Molt currently uses fixed values.
TK_VERSION = "8.6"
TCL_VERSION = "8.6"
READABLE = 2
WRITABLE = 4
EXCEPTION = 8
DONT_WAIT = 2
ALL_EVENTS = -3
FILE_EVENTS = 8
TIMER_EVENTS = 16
IDLE_EVENTS = 32
WINDOW_EVENTS = 4

_BUSYWAIT_INTERVAL_MS = 20
_FILE_HANDLER_INVALID_FILE_MSG = "argument must be an int, or have a fileno() method."


class TclError(RuntimeError):
    """Tk/Tcl operation error."""


class Tcl_Obj(str):
    """Thin Python placeholder for CPython's `_tkinter.Tcl_Obj` type."""


class TkttType:
    """Timer-token shim used by `TkappType.createtimerhandler`."""

    def __init__(self, app, token, callback):
        self._app = app
        self._token = token
        self._callback = callback
        self._deleted = False

    def deletetimerhandler(self):
        if self._deleted:
            return None
        self._deleted = True
        self._callback = None
        if self._token is not None:
            try:
                call(self._app, "after", "cancel", self._token)
            except Exception:  # noqa: BLE001
                # Timer delete is idempotent even if the runtime already fired it.
                pass
            self._token = None
        return None

    def __repr__(self):
        status = ", handler deleted" if self._deleted else ""
        return f"<tktimertoken at {hex(id(self))}{status}>"


def _coerce_int(value, *, label):
    try:
        return int(value)
    except Exception as exc:  # noqa: BLE001
        raise TypeError(f"{label} must be an integer") from exc


def _normalize_delay_ms(delay_ms):
    delay = _coerce_int(delay_ms, label="after delay")
    if delay < 0:
        raise ValueError("after delay must be >= 0")
    return delay


def _normalize_file_descriptor(file):
    if isinstance(file, int):
        fd = file
    else:
        fileno = getattr(file, "fileno", None)
        if not callable(fileno):
            raise TypeError(_FILE_HANDLER_INVALID_FILE_MSG)
        fd = fileno()
    try:
        fd = int(fd)
    except Exception as exc:  # noqa: BLE001
        raise TypeError(_FILE_HANDLER_INVALID_FILE_MSG) from exc
    if fd < 0:
        raise ValueError(f"file descriptor cannot be a negative integer ({fd})")
    return fd


def _require_filehandler_supported(op_name):
    if sys.platform == "win32":
        raise NotImplementedError(f"{op_name} is not available on Windows")


def _normalize_last_error(value):
    if value is None:
        return None
    if isinstance(value, str):
        return value
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return str(value)


def _unwrap_app(app):
    if isinstance(app, TkappType):
        return app._handle
    return app


def _flatten(seq):
    out = []
    for item in seq:
        if isinstance(item, (tuple, list)):
            out.extend(_flatten(item))
        elif item is not None:
            out.append(item)
    return tuple(out)


def _cnfmerge(cnfs):
    if isinstance(cnfs, dict):
        return cnfs
    if cnfs is None or isinstance(cnfs, str):
        return cnfs
    merged = {}
    for cfg in _flatten(cnfs):
        if isinstance(cfg, dict):
            merged.update(cfg)
    return merged


def tk_available():
    return bool(_MOLT_TK_AVAILABLE())


def has_gui_capability():
    return bool(_MOLT_CAPABILITIES_HAS("gui.window")) or bool(
        _MOLT_CAPABILITIES_HAS("gui")
    )


def has_process_spawn_capability():
    return bool(_MOLT_CAPABILITIES_HAS("process.spawn")) or bool(
        _MOLT_CAPABILITIES_HAS("process")
    )


def wantobjects():
    return True


def getbusywaitinterval():
    return _BUSYWAIT_INTERVAL_MS


def setbusywaitinterval(interval_ms):
    global _BUSYWAIT_INTERVAL_MS
    value = _coerce_int(interval_ms, label="busywait interval")
    if value < 0:
        raise ValueError("busywait interval must be >= 0")
    _BUSYWAIT_INTERVAL_MS = value


def create(
    screenName=None,
    baseName=None,
    className="Tk",
    interactive=False,
    wantobjects=True,
    useTk=True,
    sync=False,
    use=None,
    *,
    options=None,
):
    del interactive
    del wantobjects
    create_options = {
        "screenName": screenName,
        "baseName": baseName,
        "className": className,
        "useTk": bool(useTk),
        "sync": bool(sync),
        "use": use,
    }
    if options is not None:
        if not isinstance(options, dict):
            raise TypeError("options must be a dict when provided")
        create_options.update(options)
    handle = _MOLT_TK_APP_NEW(create_options)
    return TkappType(handle)


def mainloop(app):
    _MOLT_TK_MAINLOOP(_unwrap_app(app))


def dooneevent(app, flags=0):
    return bool(_MOLT_TK_DO_ONE_EVENT(_unwrap_app(app), flags))


def quit(app):
    _MOLT_TK_QUIT(_unwrap_app(app))


def after(app, delay_ms, callback):
    if not callable(callback):
        raise TypeError("after callback must be callable")
    return _MOLT_TK_AFTER(_unwrap_app(app), _normalize_delay_ms(delay_ms), callback)


def after_idle(app, callback):
    if not callable(callback):
        raise TypeError("after callback must be callable")
    return _MOLT_TK_AFTER_IDLE(_unwrap_app(app), callback)


def _normalize_after_cancel_identifier(identifier):
    if isinstance(identifier, TkttType):
        return identifier._token
    token = getattr(identifier, "_token", None)
    if token is not None:
        return token
    return identifier


def after_cancel(app, identifier):
    if not identifier:
        raise ValueError(
            "id must be a valid identifier returned from after or after_idle"
        )

    token = _normalize_after_cancel_identifier(identifier)
    if isinstance(identifier, TkttType):
        identifier._deleted = True
        identifier._callback = None
        identifier._token = None

    if token in (None, ""):
        raise ValueError(
            "id must be a valid identifier returned from after or after_idle"
        )

    _MOLT_TK_AFTER_CANCEL(_unwrap_app(app), token)
    return None


def after_info(app, identifier=None):
    token = (
        None if identifier is None else _normalize_after_cancel_identifier(identifier)
    )
    if token is None:
        return tuple(_MOLT_TK_AFTER_INFO(_unwrap_app(app), None))
    return tuple(_MOLT_TK_AFTER_INFO(_unwrap_app(app), token))


def createtimerhandler(app, milliseconds, callback):
    if not callable(callback):
        raise TypeError("bad argument list")
    delay_ms = _normalize_delay_ms(milliseconds)
    state = {"token": None, "fired_early": False}

    def wrapped():
        token_obj = state["token"]
        if token_obj is None:
            state["fired_early"] = True
            return callback()
        if token_obj._deleted:
            return None
        token_obj._deleted = True
        token_obj._token = None
        cb = token_obj._callback
        token_obj._callback = None
        if cb is None:
            return None
        return cb()

    token = _MOLT_TK_AFTER(_unwrap_app(app), delay_ms, wrapped)
    timer = TkttType(app, token, callback)
    state["token"] = timer
    if state["fired_early"]:
        timer._deleted = True
        timer._token = None
        timer._callback = None
    return timer


def call(app, *argv):
    return _MOLT_TK_CALL(_unwrap_app(app), list(argv))


def trace_add(app, variable_name, mode, callback):
    if not callable(callback):
        raise TypeError("trace callback must be callable")
    return _MOLT_TK_TRACE_ADD(_unwrap_app(app), str(variable_name), str(mode), callback)


def trace_remove(app, variable_name, mode, cbname):
    _MOLT_TK_TRACE_REMOVE(_unwrap_app(app), str(variable_name), str(mode), str(cbname))
    return None


def trace_clear(app, variable_name):
    _MOLT_TK_TRACE_CLEAR(_unwrap_app(app), str(variable_name))
    return None


def trace_info(app, variable_name):
    return tuple(_MOLT_TK_TRACE_INFO(_unwrap_app(app), str(variable_name)))


def wait_variable(app, variable_name):
    return _MOLT_TK_TKWAIT_VARIABLE(_unwrap_app(app), str(variable_name))


def wait_window(app, target):
    return _MOLT_TK_TKWAIT_WINDOW(_unwrap_app(app), str(target))


def wait_visibility(app, target):
    return _MOLT_TK_TKWAIT_VISIBILITY(_unwrap_app(app), str(target))


def bind_register(app, target_name, sequence, callback, add_prefix):
    if add_prefix not in ("", "+"):
        raise TypeError("bind add prefix must be '' or '+'")
    if not callable(callback):
        raise TypeError("bind callback must be callable")
    return _MOLT_TK_BIND_CALLBACK_REGISTER(
        _unwrap_app(app),
        str(target_name),
        str(sequence),
        callback,
        add_prefix,
    )


def bind_unregister(app, target_name, sequence, command_name):
    _MOLT_TK_BIND_CALLBACK_UNREGISTER(
        _unwrap_app(app),
        str(target_name),
        str(sequence),
        str(command_name),
    )
    return None


def widget_bind_register(app, widget_path, bind_target, sequence, callback, add_prefix):
    if add_prefix not in ("", "+"):
        raise TypeError("bind add prefix must be '' or '+'")
    if not callable(callback):
        raise TypeError("tag_bind callback must be callable")
    return _MOLT_TK_WIDGET_BIND_CALLBACK_REGISTER(
        _unwrap_app(app),
        str(widget_path),
        str(bind_target),
        str(sequence),
        callback,
        add_prefix,
    )


def widget_bind_unregister(app, widget_path, bind_target, sequence, command_name):
    _MOLT_TK_WIDGET_BIND_CALLBACK_UNREGISTER(
        _unwrap_app(app),
        str(widget_path),
        str(bind_target),
        str(sequence),
        str(command_name),
    )
    return None


def text_tag_bind_register(app, widget_path, tagname, sequence, callback, add_prefix):
    if add_prefix not in ("", "+"):
        raise TypeError("bind add prefix must be '' or '+'")
    if not callable(callback):
        raise TypeError("tag_bind callback must be callable")
    return _MOLT_TK_TEXT_TAG_BIND_CALLBACK_REGISTER(
        _unwrap_app(app),
        str(widget_path),
        str(tagname),
        str(sequence),
        callback,
        add_prefix,
    )


def text_tag_bind_unregister(app, widget_path, tagname, sequence, command_name):
    _MOLT_TK_TEXT_TAG_BIND_CALLBACK_UNREGISTER(
        _unwrap_app(app),
        str(widget_path),
        str(tagname),
        str(sequence),
        str(command_name),
    )
    return None


def bind_script_remove_command(script, command_name):
    if not isinstance(script, str):
        script = str(script)
    if not isinstance(command_name, str):
        command_name = str(command_name)
    return _MOLT_TK_BIND_SCRIPT_REMOVE_COMMAND(script, command_name)


def treeview_tag_bind_register(
    app,
    treeview_path,
    tagname,
    sequence,
    callback,
):
    if not callable(callback):
        raise TypeError("tag_bind callback must be callable")
    return _MOLT_TK_TREEVIEW_TAG_BIND_CALLBACK_REGISTER(
        _unwrap_app(app),
        str(treeview_path),
        str(tagname),
        str(sequence),
        callback,
    )


def treeview_tag_bind_unregister(app, treeview_path, tagname, sequence, command_name):
    _MOLT_TK_TREEVIEW_TAG_BIND_CALLBACK_UNREGISTER(
        _unwrap_app(app),
        str(treeview_path),
        str(tagname),
        str(sequence),
        str(command_name),
    )
    return None


def bind_command(app, name, callback):
    if not callable(callback):
        raise TypeError("bind_command callback must be callable")
    _MOLT_TK_BIND_COMMAND(_unwrap_app(app), name, callback)


def destroy_widget(app, widget_path):
    _MOLT_TK_DESTROY_WIDGET(_unwrap_app(app), widget_path)


def last_error(app):
    return _normalize_last_error(_MOLT_TK_LAST_ERROR(_unwrap_app(app)))


def createcommand(app, name, callback):
    if not isinstance(name, str):
        name = str(name)
    bind_command(app, name, callback)


def deletecommand(app, name):
    if not isinstance(name, str):
        name = str(name)
    _MOLT_TK_UNBIND_COMMAND(_unwrap_app(app), name)
    return None


def getboolean(value):
    try:
        return bool(_MOLT_TK_GETBOOLEAN(value))
    except Exception as exc:  # noqa: BLE001
        raise TclError(str(exc)) from exc


def getint(value):
    return _coerce_int(value, label="integer value")


def getdouble(value):
    try:
        return float(_MOLT_TK_GETDOUBLE(value))
    except Exception as exc:  # noqa: BLE001
        raise TclError(f'invalid floating-point value "{value}"') from exc


def splitlist(value):
    return tuple(_MOLT_TK_SPLITLIST(value))


def getvar(app, name):
    return call(app, "set", name)


def setvar(app, name, value):
    return call(app, "set", name, value)


def unsetvar(app, name):
    return call(app, "unset", name)


def globalgetvar(app, name):
    return getvar(app, name)


def globalsetvar(app, name, value):
    return setvar(app, name, value)


def globalunsetvar(app, name):
    return unsetvar(app, name)


def createfilehandler(app, file, mask, callback):
    _require_filehandler_supported("createfilehandler")
    if not isinstance(app, TkappType):
        raise NotImplementedError(
            "createfilehandler is only supported for TkappType handles"
        )
    if not callable(callback):
        raise TypeError("bad argument list")
    fd = _normalize_file_descriptor(file)
    event_mask = _coerce_int(mask, label="filehandler mask")
    _MOLT_TK_FILEHANDLER_CREATE(_unwrap_app(app), fd, event_mask, callback, file)
    return None


def deletefilehandler(app, file):
    _require_filehandler_supported("deletefilehandler")
    if not isinstance(app, TkappType):
        raise NotImplementedError(
            "deletefilehandler is only supported for TkappType handles"
        )
    fd = _normalize_file_descriptor(file)
    _MOLT_TK_FILEHANDLER_DELETE(_unwrap_app(app), fd)
    return None


class TkappType:
    """Python shell for a Tk app handle backed by Rust intrinsics."""

    def __init__(self, handle):
        self._handle = handle
        self._trace = None
        self._dispatching = False

    def call(self, *argv):
        return call(self, *argv)

    def eval(self, script):
        return call(self, "eval", script)

    def evalfile(self, filename):
        return call(self, "source", filename)

    def exprstring(self, expression):
        return call(self, "expr", expression)

    def exprlong(self, expression):
        return getint(self.exprstring(expression))

    def exprdouble(self, expression):
        return getdouble(self.exprstring(expression))

    def exprboolean(self, expression):
        return getboolean(self.exprstring(expression))

    def mainloop(self, threshold=0):
        del threshold
        mainloop(self)

    def dooneevent(self, flags=0):
        return dooneevent(self, flags)

    def quit(self):
        quit(self)

    def after(self, delay_ms, callback):
        return after(self, delay_ms, callback)

    def after_idle(self, callback):
        return after_idle(self, callback)

    def after_cancel(self, identifier):
        return after_cancel(self, identifier)

    def after_info(self, identifier=None):
        return after_info(self, identifier)

    def createtimerhandler(self, milliseconds, callback):
        return createtimerhandler(self, milliseconds, callback)

    def createcommand(self, name, callback):
        createcommand(self, name, callback)

    def deletecommand(self, name):
        deletecommand(self, name)

    def destroy_widget(self, widget_path):
        destroy_widget(self, widget_path)

    def last_error(self):
        return last_error(self)

    def getboolean(self, value):
        return getboolean(value)

    def getint(self, value):
        return getint(value)

    def getdouble(self, value):
        return getdouble(value)

    def splitlist(self, value):
        return splitlist(value)

    def getvar(self, name):
        return getvar(self, name)

    def setvar(self, name, value):
        return setvar(self, name, value)

    def unsetvar(self, name):
        return unsetvar(self, name)

    def globalgetvar(self, name):
        return globalgetvar(self, name)

    def globalsetvar(self, name, value):
        return globalsetvar(self, name, value)

    def globalunsetvar(self, name):
        return globalunsetvar(self, name)

    def tkappname(self):
        return "molt"

    def wantobjects(self):
        return wantobjects()

    def loadtk(self):
        return call(self, "loadtk")

    def record(self, script):
        if not isinstance(script, str):
            script = str(script)
        return call(self, "history", "add", script, "exec")

    def adderrorinfo(self, msg):
        if not isinstance(msg, str):
            msg = str(msg)
        _MOLT_TK_ERRORINFO_APPEND(_unwrap_app(self), msg)
        return None

    def settrace(self, func):
        self._trace = None if func is None else func
        return None

    def gettrace(self):
        return self._trace

    def willdispatch(self):
        self._dispatching = True
        return None

    def createfilehandler(self, file, mask, callback):
        return createfilehandler(self, file, mask, callback)

    def deletefilehandler(self, file):
        return deletefilehandler(self, file)

    def interpaddr(self):
        return id(self._handle)


__all__ = [
    "ALL_EVENTS",
    "DONT_WAIT",
    "EXCEPTION",
    "FILE_EVENTS",
    "IDLE_EVENTS",
    "READABLE",
    "TCL_VERSION",
    "TIMER_EVENTS",
    "TK_VERSION",
    "TclError",
    "Tcl_Obj",
    "TkttType",
    "TkappType",
    "WINDOW_EVENTS",
    "WRITABLE",
    "_cnfmerge",
    "_flatten",
    "after",
    "after_cancel",
    "after_info",
    "after_idle",
    "bind_register",
    "bind_script_remove_command",
    "bind_unregister",
    "bind_command",
    "call",
    "create",
    "createcommand",
    "createfilehandler",
    "createtimerhandler",
    "deletecommand",
    "deletefilehandler",
    "destroy_widget",
    "dooneevent",
    "getboolean",
    "getbusywaitinterval",
    "getdouble",
    "getint",
    "getvar",
    "globalgetvar",
    "globalsetvar",
    "globalunsetvar",
    "has_gui_capability",
    "has_process_spawn_capability",
    "last_error",
    "mainloop",
    "quit",
    "setbusywaitinterval",
    "setvar",
    "splitlist",
    "trace_add",
    "trace_clear",
    "trace_info",
    "trace_remove",
    "tk_available",
    "text_tag_bind_register",
    "text_tag_bind_unregister",
    "treeview_tag_bind_register",
    "treeview_tag_bind_unregister",
    "unsetvar",
    "wait_variable",
    "wait_visibility",
    "wait_window",
    "wantobjects",
    "widget_bind_register",
    "widget_bind_unregister",
]


def __getattr__(attr):
    raise AttributeError(f'module "{__name__}" has no attribute "{attr}"')
