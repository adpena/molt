from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"


def _run_probe(script: str) -> list[str]:
    rendered = script.replace("__STDLIB_ROOT__", repr(str(STDLIB_ROOT)))
    env = os.environ.copy()
    proc = subprocess.run(
        [sys.executable, "-c", rendered],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
        env=env,
    )
    return [line for line in proc.stdout.splitlines() if line]


_UNAVAILABLE_RUNTIME_PROBE = """
import builtins
import fnmatch as _host_fnmatch
import sys
import types as _host_types

sys.path.insert(0, __STDLIB_ROOT__)

def _runtime_unavailable(op):
    raise RuntimeError(f"tk operation unavailable ({op})")

_tk_available_probe_state = {"calls": 0}

def _tk_available():
    _tk_available_probe_state["calls"] += 1
    return False

def _types_bootstrap():
    keys = (
        "AsyncGeneratorType",
        "BuiltinFunctionType",
        "BuiltinMethodType",
        "CapsuleType",
        "CellType",
        "ClassMethodDescriptorType",
        "CodeType",
        "CoroutineType",
        "EllipsisType",
        "FrameType",
        "FunctionType",
        "GeneratorType",
        "MappingProxyType",
        "MethodType",
        "MethodDescriptorType",
        "MethodWrapperType",
        "ModuleType",
        "NoneType",
        "NotImplementedType",
        "GenericAlias",
        "GetSetDescriptorType",
        "LambdaType",
        "MemberDescriptorType",
        "SimpleNamespace",
        "TracebackType",
        "UnionType",
        "WrapperDescriptorType",
        "DynamicClassAttribute",
        "coroutine",
        "get_original_bases",
        "new_class",
        "prepare_class",
        "resolve_bases",
    )
    return {name: getattr(_host_types, name) for name in keys}

def _tk_after_info(_app=None, _identifier=None):
    return ()

def _tk_trace_clear(_app=None, _name=None):
    return None

def _tk_event_int(value):
    return int(value)

def _tk_event_build_from_args(widget_path, event_args):
    values = list(event_args)
    event = {"widget": widget_path}
    if len(values) > 8:
        event["x"] = int(values[8])
    if len(values) > 9:
        event["y"] = int(values[9])
    if len(values) > 18:
        event["delta"] = int(values[18])
    return event

def _tk_event_state_decode(value):
    return () if int(value) == 0 else (str(int(value)),)

def _tk_splitdict(value, _cut_minus=True):
    if isinstance(value, dict):
        return list(value.items())
    items = list(value)
    return [(items[idx], items[idx + 1]) for idx in range(0, len(items) - 1, 2)]

def _tk_flatten_args(seq):
    out = []
    for item in seq:
        if isinstance(item, (list, tuple)):
            out.extend(_tk_flatten_args(item))
        else:
            out.append(item)
    return out

def _tk_cnfmerge(cnfs, _fallback=None):
    merged = {}
    for cnf in cnfs or ():
        if isinstance(cnf, dict):
            merged.update(cnf)
    return merged

def _tk_normalize_option(name):
    text = str(name)
    if text.endswith("_"):
        text = text[:-1]
    text = text.replace("_", "-")
    return text if text.startswith("-") else f"-{text}"

def _tk_normalize_delay_ms(value):
    return int(value)

builtins._molt_intrinsics = {"molt_capabilities_has": lambda _name=None: True,
    "molt_stdlib_probe": lambda: True,
    "molt_fnmatch": lambda name, pat: _host_fnmatch.fnmatch(name, pat),
    "molt_fnmatchcase": lambda name, pat: _host_fnmatch.fnmatchcase(name, pat),
    "molt_fnmatch_filter": lambda names, pat, _casefold=False: _host_fnmatch.filter(list(names), pat),
    "molt_fnmatch_translate": lambda pat: _host_fnmatch.translate(pat),
    "molt_types_bootstrap": _types_bootstrap,
    "molt_tk_available": _tk_available,
    "molt_tk_app_new": lambda _opts=None: _runtime_unavailable("molt_tk_app_new"),
    "molt_tk_quit": lambda _app=None: _runtime_unavailable("molt_tk_quit"),
    "molt_tk_mainloop": lambda _app=None: _runtime_unavailable("molt_tk_mainloop"),
    "molt_tk_do_one_event": lambda _app=None, _flags=None: _runtime_unavailable("molt_tk_do_one_event"),
    "molt_tk_after": lambda _app=None, _delay=None, _fn=None: _runtime_unavailable("molt_tk_after"),
    "molt_tk_after_idle": lambda _app=None, _fn=None: _runtime_unavailable("molt_tk_after_idle"),
    "molt_tk_after_cancel": lambda _app=None, _token=None: None,
    "molt_tk_after_info": _tk_after_info,
    "molt_tk_call": lambda _app=None, _argv=None: _runtime_unavailable("molt_tk_call"),
    "molt_tk_trace_add": lambda _app=None, _name=None, _mode=None, _fn=None: _runtime_unavailable("molt_tk_trace_add"),
    "molt_tk_trace_remove": lambda _app=None, _name=None, _mode=None, _cb=None: None,
    "molt_tk_trace_clear": _tk_trace_clear,
    "molt_tk_trace_info": lambda _app=None, _name=None: (),
    "molt_tk_tkwait_variable": lambda _app=None, _name=None: _runtime_unavailable("molt_tk_tkwait_variable"),
    "molt_tk_tkwait_window": lambda _app=None, _target=None: _runtime_unavailable("molt_tk_tkwait_window"),
    "molt_tk_tkwait_visibility": lambda _app=None, _target=None: _runtime_unavailable("molt_tk_tkwait_visibility"),
    "molt_tk_bind_callback_register": lambda _app=None, _target=None, _sequence=None, _fn=None, _add=None: _runtime_unavailable("molt_tk_bind_callback_register"),
    "molt_tk_bind_callback_unregister": lambda _app=None, _target=None, _sequence=None, _command=None: None,
    "molt_tk_widget_bind_callback_register": lambda _app=None, _target=None, _sequence=None, _fn=None, _add=None: _runtime_unavailable("molt_tk_widget_bind_callback_register"),
    "molt_tk_widget_bind_callback_unregister": lambda _app=None, _target=None, _sequence=None, _command=None: None,
    "molt_tk_text_tag_bind_callback_register": lambda _app=None, _target=None, _tag=None, _sequence=None, _fn=None: _runtime_unavailable("molt_tk_text_tag_bind_callback_register"),
    "molt_tk_text_tag_bind_callback_unregister": lambda _app=None, _target=None, _tag=None, _sequence=None, _command=None: None,
    "molt_tk_treeview_tag_bind_callback_register": lambda _app=None, _path=None, _tag=None, _sequence=None, _fn=None: _runtime_unavailable("molt_tk_treeview_tag_bind_callback_register"),
    "molt_tk_treeview_tag_bind_callback_unregister": lambda _app=None, _path=None, _tag=None, _sequence=None, _command=None: None,
    "molt_tk_bind_command": lambda _app=None, _name=None, _fn=None: _runtime_unavailable("molt_tk_bind_command"),
    "molt_tk_unbind_command": lambda _app=None, _name=None: _runtime_unavailable("molt_tk_unbind_command"),
    "molt_tk_filehandler_create": lambda _app=None, _fd=None, _mask=None, _callback=None, _file=None: _runtime_unavailable("molt_tk_filehandler_create"),
    "molt_tk_filehandler_delete": lambda _app=None, _fd=None: _runtime_unavailable("molt_tk_filehandler_delete"),
    "molt_tk_destroy_widget": lambda _app=None, _w=None: _runtime_unavailable("molt_tk_destroy_widget"),
    "molt_tk_last_error": lambda _app=None: "tk runtime unavailable",
    "molt_tk_getboolean": lambda value: bool(value),
    "molt_tk_getdouble": lambda value: float(value),
    "molt_tk_splitlist": lambda value: tuple(value) if isinstance(value, (list, tuple)) else tuple(str(value).split()),
    "molt_tk_errorinfo_append": lambda _app=None, _text=None: None,
    "molt_tk_bind_script_remove_command": lambda _script=None, _command_name=None: _script,
    "molt_tk_event_subst_parse": lambda _widget=None, event_args=(): tuple(event_args),
    "molt_tk_event_int": _tk_event_int,
    "molt_tk_event_build_from_args": _tk_event_build_from_args,
    "molt_tk_event_state_decode": _tk_event_state_decode,
    "molt_tk_splitdict": _tk_splitdict,
    "molt_tk_flatten_args": _tk_flatten_args,
    "molt_tk_cnfmerge": _tk_cnfmerge,
    "molt_tk_normalize_option": _tk_normalize_option,
    "molt_tk_normalize_delay_ms": _tk_normalize_delay_ms,
    "molt_tk_hex_to_rgb": lambda _color=None: (0, 0, 0),
    "molt_tk_commondialog_show": lambda _app=None, _master=None, _command=None, _options=None: _runtime_unavailable("molt_tk_commondialog_show"),
    "molt_tk_messagebox_show": lambda _app=None, _master=None, _options=None: _runtime_unavailable("molt_tk_messagebox_show"),
    "molt_tk_filedialog_show": lambda _app=None, _master=None, _command=None, _options=None: _runtime_unavailable("molt_tk_filedialog_show"),
    "molt_tk_dialog_show": lambda _app=None, _master=None, _title=None, _text=None, _bitmap=None, _default=None, _strings=None: _runtime_unavailable("molt_tk_dialog_show"),
    "molt_tk_simpledialog_query": lambda _app=None, _parent=None, _title=None, _prompt=None, _initial=None, _kind=None, _min=None, _max=None: _runtime_unavailable("molt_tk_simpledialog_query"),
}

MODULES = [
    "tkinter.__main__",
    "tkinter.constants",
    "tkinter.colorchooser",
    "tkinter.commondialog",
    "tkinter.dialog",
    "tkinter.dnd",
    "tkinter.filedialog",
    "tkinter.font",
    "tkinter.messagebox",
    "tkinter.scrolledtext",
    "tkinter.simpledialog",
    "tkinter.tix",
]

for name in MODULES:
    try:
        __import__(name, fromlist=["*"])
    except BaseException as exc:  # noqa: BLE001
        print(f"IMPORT|{name}|error|{type(exc).__name__}|{exc}")
    else:
        print(f"IMPORT|{name}|ok")

import tkinter.__main__ as tk_main
import tkinter.colorchooser as tk_colorchooser
import tkinter.commondialog as tk_commondialog
import tkinter.dialog as tk_dialog
import tkinter.dnd as tk_dnd
import tkinter.filedialog as tk_filedialog
import tkinter.font as tk_font
import tkinter.messagebox as tk_messagebox
import tkinter.scrolledtext as tk_scrolledtext
import tkinter.simpledialog as tk_simpledialog
import tkinter.tix as tk_tix
import tkinter as tk_root
import _tkinter as tk_core

print(f"CHECK|_tkinter_tk_available_false|{tk_core.tk_available() is False}")
print(f"CHECK|tkinter_tk_available_false|{tk_root.tk_available() is False}")


class _Event:
    widget = None


class _ProbeDialog(tk_commondialog.Dialog):
    command = "tk_messageBox"


CALLS = [
    ("main", tk_main.main, ()),
    ("askcolor", tk_colorchooser.askcolor, ()),
    ("commondialog_show", _ProbeDialog().show, ()),
    ("dialog_show", tk_dialog.Dialog(text="probe", strings=("OK",)).show, ()),
    ("dnd_start", tk_dnd.dnd_start, ("source", _Event())),
    ("askopenfilename", tk_filedialog.askopenfilename, ()),
    ("font_families", tk_font.families, ()),
    ("showinfo", tk_messagebox.showinfo, ("title", "message")),
    ("scrolledtext", tk_scrolledtext.ScrolledText, ()),
    ("askstring", tk_simpledialog.askstring, ("title", "prompt")),
    ("tixCommand", tk_tix.tixCommand, ()),
]

for label, fn, args in CALLS:
    before = _tk_available_probe_state["calls"]
    try:
        fn(*args)
    except BaseException as exc:  # noqa: BLE001
        print(f"CALL|{label}|error|{type(exc).__name__}|{exc}")
    else:
        print(f"CALL|{label}|ok")
    finally:
        after = _tk_available_probe_state["calls"]
        print(f"TK_AVAILABLE_CALLS|{label}|{after - before}")

for label, gate_name, probe_name in (
    ("gui", "_require_gui_window_capability", "_HAS_GUI_CAPABILITY"),
    ("process", "_require_process_spawn_capability", "_HAS_PROCESS_SPAWN_CAPABILITY"),
):
    gate = getattr(tk_root, gate_name, None)
    probe = getattr(tk_root, probe_name, None)
    capability_probe = getattr(tk_root, "_MOLT_CAPABILITIES_HAS", None)
    if not callable(gate) or not callable(probe):
        print(f"GATE|{label}|missing")
        continue
    if not callable(capability_probe):
        print(f"GATE|{label}|missing_capability_probe")
        continue
    try:
        setattr(tk_root, probe_name, lambda: False)
        setattr(tk_root, "_MOLT_CAPABILITIES_HAS", lambda _name=None: False)
        try:
            gate()
        except BaseException as exc:  # noqa: BLE001
            print(f"GATE|{label}|error|{type(exc).__name__}|{exc}")
        else:
            print(f"GATE|{label}|ok")
    finally:
        setattr(tk_root, probe_name, probe)
        setattr(tk_root, "_MOLT_CAPABILITIES_HAS", capability_probe)

try:
    tk_root.Tk(useTk=False)
except BaseException as exc:  # noqa: BLE001
    print(f"UNAVAILABLE|tk_constructor_use_tk_false|error|{type(exc).__name__}|{exc}")
else:
    print("UNAVAILABLE|tk_constructor_use_tk_false|ok")
"""

_HEADLESS_RUNTIME_PROBE = """
import builtins
import fnmatch as _host_fnmatch
import sys
import types as _host_types

sys.path.insert(0, __STDLIB_ROOT__)

def _capabilities_has(name=None):
    return name in {"gui.window", "gui", "process.spawn", "process"}

def _types_bootstrap():
    keys = (
        "AsyncGeneratorType",
        "BuiltinFunctionType",
        "BuiltinMethodType",
        "CapsuleType",
        "CellType",
        "ClassMethodDescriptorType",
        "CodeType",
        "CoroutineType",
        "EllipsisType",
        "FrameType",
        "FunctionType",
        "GeneratorType",
        "MappingProxyType",
        "MethodType",
        "MethodDescriptorType",
        "MethodWrapperType",
        "ModuleType",
        "NoneType",
        "NotImplementedType",
        "GenericAlias",
        "GetSetDescriptorType",
        "LambdaType",
        "MemberDescriptorType",
        "SimpleNamespace",
        "TracebackType",
        "UnionType",
        "WrapperDescriptorType",
        "DynamicClassAttribute",
        "coroutine",
        "get_original_bases",
        "new_class",
        "prepare_class",
        "resolve_bases",
    )
    return {name: getattr(_host_types, name) for name in keys}

def _app_new(_opts=None):
    create_options = dict(_opts or {})
    return {
        "vars": {},
        "commands": {},
        "fileevents": {},
        "wm_protocols": {},
        "tree_tag_bindings": {},
        "calls": [],
        "last_error": None,
        "create_options": create_options,
    }

def _to_fd(file_obj):
    if isinstance(file_obj, int):
        return file_obj
    return int(file_obj.fileno())

def _dispatch_fileevent(app, file_obj, event_name):
    fd = _to_fd(file_obj)
    command = app["fileevents"].get(fd, {}).get(str(event_name))
    if not command:
        return False
    callback = app["commands"].get(command)
    if callback is None:
        return False
    callback()
    return True

_SUPPORTED_COMMONDIALOG_COMMANDS = {
    "tk_messageBox",
    "tk_getOpenFile",
    "tk_getSaveFile",
    "tk_chooseDirectory",
    "tk_chooseColor",
}

def _commondialog_has_option(options, option_name):
    want = str(option_name).lower()
    idx = 0
    while idx + 1 < len(options):
        name = str(options[idx])
        if not name.startswith("-"):
            name = f"-{name}"
        if name.lower() == want:
            return True
        idx += 2
    return False

def _tk_call(app, argv):
    app["calls"].append(tuple(argv))
    if not argv:
        return None
    op = str(argv[0])
    if op == "set":
        if len(argv) == 2:
            return app["vars"][str(argv[1])]
        if len(argv) == 3:
            app["vars"][str(argv[1])] = argv[2]
            return argv[2]
        raise RuntimeError("invalid set arity")
    if op == "unset":
        if len(argv) != 2:
            raise RuntimeError("invalid unset arity")
        app["vars"].pop(str(argv[1]), None)
        return ""
    if op == "rename" and len(argv) == 3 and argv[2] == "":
        name = str(argv[1])
        if name not in app["commands"]:
            raise RuntimeError(f'invalid command name "{name}"')
        app["commands"].pop(name, None)
        return ""
    if op == "fileevent" and len(argv) == 4:
        fd = _to_fd(argv[1])
        event_name = str(argv[2])
        command_name = str(argv[3])
        events = app["fileevents"].setdefault(fd, {})
        if command_name:
            events[event_name] = command_name
        else:
            events.pop(event_name, None)
            if not events:
                app["fileevents"].pop(fd, None)
        return ""
    if op == "after":
        return "after#stub"
    if op == "wm" and len(argv) >= 4:
        wm_subcommand = str(argv[1])
        target = str(argv[2])
        if wm_subcommand == "protocol":
            key = (target, str(argv[3]))
            if len(argv) == 4:
                return app["wm_protocols"].get(key, "")
            handler = str(argv[4])
            if handler:
                app["wm_protocols"][key] = handler
            else:
                app["wm_protocols"].pop(key, None)
            return ""
    if op == "tkwait" and len(argv) >= 3 and str(argv[1]) == "window":
        for command_name, callback in tuple(app["commands"].items()):
            if "simpledialog_ok" in str(command_name):
                callback()
                break
        return ""
    if op == "tk_messageBox":
        return "ok"
    if op in {"tk_getOpenFile", "tk_getSaveFile", "tk_chooseDirectory", "tk_chooseColor"}:
        return ""
    if op == "ttk::style":
        subcommand = str(argv[1]) if len(argv) >= 2 else ""
        if subcommand == "configure":
            if len(argv) == 3:
                return ("-padding", "4")
            if len(argv) == 4:
                return "4"
            return ""
        if subcommand == "map":
            if len(argv) == 3:
                return ("-foreground", ("active", "blue"))
            if len(argv) == 4:
                return (("active", "blue"),)
            return ""
        if subcommand == "lookup":
            return "blue"
        if subcommand == "layout":
            if len(argv) == 3:
                return ()
            return ""
        if subcommand == "element":
            op_name = str(argv[2]) if len(argv) >= 3 else ""
            if op_name == "names":
                return ("Probe.indicator",)
            if op_name == "options":
                return ("-foreground",)
            return ""
        if subcommand == "theme":
            op_name = str(argv[2]) if len(argv) >= 3 else ""
            if op_name == "names":
                return ("default", "probe_theme")
            if op_name == "use":
                if len(argv) == 3:
                    return "probe_theme"
                return ""
            return ""
    if len(argv) >= 2 and str(argv[1]) == "exists":
        return "1"
    if len(argv) >= 2 and str(argv[1]) == "bbox":
        return "1 2 3 4"
    if len(argv) >= 2 and str(argv[1]) == "index":
        return "7"
    if len(argv) >= 2 and str(argv[1]) == "set":
        if len(argv) == 3:
            return ("#0", "row-text", "value", "row-value")
        if len(argv) == 4:
            return "row-value"
        return ""
    if len(argv) >= 2 and str(argv[1]) == "selection":
        if len(argv) == 2:
            return ("row1",)
        return ""
    if len(argv) >= 3 and str(argv[1]) == "tag" and str(argv[2]) == "has":
        if len(argv) == 4:
            return ("row1", "row2")
        return "1"
    if len(argv) >= 3 and str(argv[1]) == "tag" and str(argv[2]) == "bind":
        tagname = str(argv[3]) if len(argv) >= 4 else ""
        sequence = str(argv[4]) if len(argv) >= 5 else ""
        key = (str(op), tagname, sequence)
        if len(argv) == 5:
            return app["tree_tag_bindings"].get(key, "")
        if len(argv) >= 6:
            app["tree_tag_bindings"][key] = str(argv[5])
            return ""
    return tuple(argv)

def _tk_bind_command(app, name, callback):
    app["commands"][str(name)] = callback

def _tk_unbind_command(app, name):
    name = str(name)
    if name in app["commands"]:
        app["commands"].pop(name, None)
        app["calls"].append(("rename", name, ""))
        return None
    for fd, events in tuple(app["fileevents"].items()):
        found_event = None
        for event_name, command_name in tuple(events.items()):
            if command_name == name:
                found_event = event_name
                break
        if found_event is None:
            continue
        events.pop(found_event, None)
        if not events:
            app["fileevents"].pop(fd, None)
        app["commands"].pop(name, None)
        app["calls"].append(("rename", name, ""))
        return None
    raise RuntimeError(f'invalid command name "{name}"')

def _tk_after(_app, delay_ms, callback):
    callback()
    return f"after#{int(delay_ms)}"

_FILE_EVENT_FLAGS = (
    (2, "readable"),
    (4, "writable"),
    (8, "exception"),
)

def _filehandler_command_name(fd, event_name):
    return f"::__molt_filehandler_{int(fd)}_{event_name}"

def _tk_filehandler_create(app, fd, mask, callback, file_obj):
    if not callable(callback):
        raise TypeError("bad argument list")
    fd = int(fd)
    mask = int(mask)
    _tk_filehandler_delete(app, fd)
    events = {}
    for event_mask, event_name in _FILE_EVENT_FLAGS:
        if mask & event_mask:
            command_name = _filehandler_command_name(fd, event_name)

            def wrapped(*_args, _file=file_obj, _mask=event_mask, _callback=callback):
                return _callback(_file, _mask)

            app["commands"][command_name] = wrapped
            events[event_name] = command_name
    if events:
        app["fileevents"][fd] = events
    return None

def _tk_filehandler_delete(app, fd):
    fd = int(fd)
    events = app["fileevents"].pop(fd, None)
    if not events:
        return None
    for command_name in events.values():
        app["commands"].pop(command_name, None)
    return None

def _tk_dialog_show(_app, _master, _title, _text, _bitmap, default_index, strings):
    labels = tuple(strings or ())
    try:
        selected = int(default_index)
    except Exception:  # noqa: BLE001
        selected = 0
    if selected < 0:
        selected = 0
    if labels:
        selected = min(selected, len(labels) - 1)
    return selected

def _tk_commondialog_show(_app, _master, command, options):
    command_name = str(command)
    if command_name not in _SUPPORTED_COMMONDIALOG_COMMANDS:
        raise RuntimeError(f'unsupported commondialog command "{command_name}"')
    normalized_options = tuple(options or ())
    argv = [command_name]
    if _master not in (None, "") and not _commondialog_has_option(
        normalized_options, "-parent"
    ):
        argv.extend(["-parent", str(_master)])
    argv.extend(normalized_options)
    return _tk_call(_app, argv)

def _tk_messagebox_show(_app, _master, options):
    return _tk_commondialog_show(_app, _master, "tk_messageBox", options)

def _tk_filedialog_show(_app, _master, command, options):
    return _tk_commondialog_show(_app, _master, command, options)

def _tk_simpledialog_query(
    _app,
    _parent,
    _title,
    _prompt,
    initial_value,
    query_kind,
    min_value,
    max_value,
):
    text = "" if initial_value is None else str(initial_value)
    if query_kind == "string":
        return text
    if query_kind == "int":
        try:
            value = int(text)
        except Exception:  # noqa: BLE001
            return None
        if min_value is not None and value < int(min_value):
            return None
        if max_value is not None and value > int(max_value):
            return None
        return value
    if query_kind == "float":
        try:
            value = float(text)
        except Exception:  # noqa: BLE001
            return None
        if min_value is not None and value < float(min_value):
            return None
        if max_value is not None and value > float(max_value):
            return None
        return value
    raise RuntimeError(f"unsupported query kind: {query_kind}")

_command_serial = {"value": 0}


def _next_command_name(prefix):
    _command_serial["value"] += 1
    return f"{prefix}{_command_serial['value']}"


def _tk_after_idle(_app, callback):
    callback()
    return "after#idle"


def _tk_after_cancel(_app, _token):
    return None


def _tk_after_info(_app=None, _identifier=None):
    return ()


def _tk_trace_add(app, _name, _mode, callback):
    command_name = _next_command_name("::__molt_trace_")
    app["commands"][command_name] = callback
    return command_name


def _tk_trace_remove(app, _name, _mode, callback_name):
    app["commands"].pop(str(callback_name), None)
    return None


def _tk_trace_clear(_app=None, _name=None):
    return None


def _tk_trace_info(_app, _name):
    return ()


def _tk_tkwait_variable(_app, _name):
    return None


def _tk_tkwait_window(_app, _target):
    return None


def _tk_tkwait_visibility(_app, _target):
    return None


def _tk_bind_callback_register(app, _target_name, _sequence, callback, _add_prefix):
    command_name = _next_command_name("::__molt_bind_")
    app["commands"][command_name] = callback
    return command_name


def _tk_bind_callback_unregister(app, _target_name, _sequence, command_name):
    app["commands"].pop(str(command_name), None)
    return None


def _tk_treeview_tag_bind_callback_register(app, treeview_path, tagname, sequence, callback):
    command_name = _next_command_name("::__molt_ttk_tag_")
    app["commands"][command_name] = callback
    app["tree_tag_bindings"][(str(treeview_path), str(tagname), str(sequence))] = command_name
    return command_name


def _tk_treeview_tag_bind_callback_unregister(
    app, treeview_path, tagname, sequence, command_name
):
    key = (str(treeview_path), str(tagname), str(sequence))
    if app["tree_tag_bindings"].get(key) == str(command_name):
        app["tree_tag_bindings"][key] = ""
    app["commands"].pop(str(command_name), None)
    return None


def _tk_getboolean(value):
    if isinstance(value, str):
        lowered = value.strip().lower()
        if lowered in {"1", "true", "yes", "on"}:
            return True
        if lowered in {"0", "false", "no", "off", ""}:
            return False
    return bool(value)


def _tk_getdouble(value):
    return float(value)


def _tk_splitlist(value):
    if isinstance(value, (tuple, list)):
        return tuple(value)
    text = str(value).strip()
    if not text:
        return ()
    return tuple(text.split())


def _tk_errorinfo_append(app, text):
    current = app["vars"].get("errorInfo")
    if current:
        app["vars"]["errorInfo"] = f"{current}\\n{text}"
    else:
        app["vars"]["errorInfo"] = str(text)
    return None


def _tk_event_subst_parse(_widget_path, event_args):
    return tuple(event_args)


def _tk_after_info(_app=None, _identifier=None):
    return ()


def _tk_trace_clear(_app=None, _name=None):
    return None


def _tk_event_int(value):
    return int(value)


def _tk_event_build_from_args(widget_path, event_args):
    values = list(event_args)
    event = {"widget": widget_path}
    if len(values) > 8:
        event["x"] = int(values[8])
    if len(values) > 9:
        event["y"] = int(values[9])
    if len(values) > 18:
        event["delta"] = int(values[18])
    return event


def _tk_event_state_decode(value):
    return () if int(value) == 0 else (str(int(value)),)


def _tk_splitdict(value, _cut_minus=True):
    if isinstance(value, dict):
        return list(value.items())
    items = list(value)
    return [(items[idx], items[idx + 1]) for idx in range(0, len(items) - 1, 2)]


def _tk_flatten_args(seq):
    out = []
    for item in seq:
        if isinstance(item, (list, tuple)):
            out.extend(_tk_flatten_args(item))
        else:
            out.append(item)
    return out


def _tk_cnfmerge(cnfs, _fallback=None):
    merged = {}
    for cnf in cnfs or ():
        if isinstance(cnf, dict):
            merged.update(cnf)
    return merged


def _tk_normalize_option(name):
    text = str(name)
    if text.endswith("_"):
        text = text[:-1]
    text = text.replace("_", "-")
    return text if text.startswith("-") else f"-{text}"


def _tk_normalize_delay_ms(value):
    return int(value)


def _tk_convert_stringval(value):
    text = str(value)
    lowered = text.lower()
    if lowered == "true":
        return True
    if lowered == "false":
        return False
    try:
        return int(text)
    except ValueError:
        return text


def _tk_hex_to_rgb(color):
    text = str(color).lstrip("#")
    if len(text) == 3:
        text = "".join(ch * 2 for ch in text)
    if len(text) != 6:
        raise ValueError("expected #RRGGBB color")
    return tuple(int(text[idx : idx + 2], 16) * 257 for idx in (0, 2, 4))


builtins._molt_intrinsics = {
    "molt_stdlib_probe": lambda: True,
    "molt_fnmatch": lambda name, pat: _host_fnmatch.fnmatch(name, pat),
    "molt_fnmatchcase": lambda name, pat: _host_fnmatch.fnmatchcase(name, pat),
    "molt_fnmatch_filter": lambda names, pat, _casefold=False: _host_fnmatch.filter(list(names), pat),
    "molt_fnmatch_translate": lambda pat: _host_fnmatch.translate(pat),
    "molt_types_bootstrap": _types_bootstrap,
    "molt_capabilities_has": _capabilities_has,
    "molt_tk_available": lambda: True,
    "molt_tk_app_new": _app_new,
    "molt_tk_quit": lambda _app=None: None,
    "molt_tk_mainloop": lambda _app=None: None,
    "molt_tk_do_one_event": lambda _app=None, _flags=None: False,
    "molt_tk_after": _tk_after,
    "molt_tk_after_idle": _tk_after_idle,
    "molt_tk_after_cancel": _tk_after_cancel,
    "molt_tk_after_info": _tk_after_info,
    "molt_tk_call": _tk_call,
    "molt_tk_trace_add": _tk_trace_add,
    "molt_tk_trace_remove": _tk_trace_remove,
    "molt_tk_trace_clear": _tk_trace_clear,
    "molt_tk_trace_info": _tk_trace_info,
    "molt_tk_tkwait_variable": _tk_tkwait_variable,
    "molt_tk_tkwait_window": _tk_tkwait_window,
    "molt_tk_tkwait_visibility": _tk_tkwait_visibility,
    "molt_tk_bind_callback_register": _tk_bind_callback_register,
    "molt_tk_bind_callback_unregister": _tk_bind_callback_unregister,
    "molt_tk_widget_bind_callback_register": _tk_bind_callback_register,
    "molt_tk_widget_bind_callback_unregister": _tk_bind_callback_unregister,
    "molt_tk_text_tag_bind_callback_register": _tk_bind_callback_register,
    "molt_tk_text_tag_bind_callback_unregister": _tk_bind_callback_unregister,
    "molt_tk_treeview_tag_bind_callback_register": _tk_treeview_tag_bind_callback_register,
    "molt_tk_treeview_tag_bind_callback_unregister": _tk_treeview_tag_bind_callback_unregister,
    "molt_tk_bind_command": _tk_bind_command,
    "molt_tk_unbind_command": _tk_unbind_command,
    "molt_tk_filehandler_create": _tk_filehandler_create,
    "molt_tk_filehandler_delete": _tk_filehandler_delete,
    "molt_tk_destroy_widget": lambda _app=None, _w=None: None,
    "molt_tk_last_error": lambda app=None: None if app is None else app.get("last_error"),
    "molt_tk_getboolean": _tk_getboolean,
    "molt_tk_getdouble": _tk_getdouble,
    "molt_tk_splitlist": _tk_splitlist,
    "molt_tk_errorinfo_append": _tk_errorinfo_append,
    "molt_tk_bind_script_remove_command": lambda _script=None, _command_name=None: _script,
    "molt_tk_event_subst_parse": _tk_event_subst_parse,
    "molt_tk_event_int": _tk_event_int,
    "molt_tk_event_build_from_args": _tk_event_build_from_args,
    "molt_tk_event_state_decode": _tk_event_state_decode,
    "molt_tk_splitdict": _tk_splitdict,
    "molt_tk_flatten_args": _tk_flatten_args,
    "molt_tk_cnfmerge": _tk_cnfmerge,
    "molt_tk_normalize_option": _tk_normalize_option,
    "molt_tk_normalize_delay_ms": _tk_normalize_delay_ms,
    "molt_tk_convert_stringval": _tk_convert_stringval,
    "molt_tk_hex_to_rgb": _tk_hex_to_rgb,
    "molt_tk_commondialog_show": _tk_commondialog_show,
    "molt_tk_messagebox_show": _tk_messagebox_show,
    "molt_tk_filedialog_show": _tk_filedialog_show,
    "molt_tk_dialog_show": _tk_dialog_show,
    "molt_tk_simpledialog_query": _tk_simpledialog_query,
}

import _tkinter
import tkinter
import tkinter.colorchooser as colorchooser
import tkinter.commondialog as commondialog
import tkinter.dialog as dialog_module
import tkinter.filedialog as filedialog
import tkinter.messagebox as messagebox
import tkinter.simpledialog as simpledialog
import tkinter.ttk as ttk

checks = {}
checks["tk_available"] = _tkinter.tk_available() is True
checks["tkinter_tk_available"] = tkinter.tk_available() is True

app = _tkinter.create(useTk=False)
checks["create_use_tk_false_forwarded"] = app._handle["create_options"].get("useTk") is False
_tkinter.setvar(app, "phase0", "value")
checks["set_get_roundtrip"] = _tkinter.getvar(app, "phase0") == "value"

_tkinter.createcommand(app, "phase0_cmd", lambda: None)
_tkinter.deletecommand(app, "phase0_cmd")
checks["deletecommand_runtime_notified"] = ("rename", "phase0_cmd", "") in app._handle["calls"]

app._handle["last_error"] = "headless runtime error"
checks["last_error_passthrough"] = _tkinter.last_error(app) == "headless runtime error"
after_events = []
after_token = _tkinter.after(app, 7, lambda: after_events.append("fired"))
checks["after_callback_invoked"] = after_token == "after#7" and after_events == ["fired"]
timer_events = []
timer_token = app.createtimerhandler(11, lambda: timer_events.append("timer-fired"))
checks["createtimerhandler_returns_tktt"] = type(timer_token).__name__ == "TkttType"
checks["createtimerhandler_callback_invoked"] = timer_events == ["timer-fired"]
checks["createtimerhandler_repr_marks_deleted"] = "handler deleted" in repr(timer_token)
cancel_calls_before = [call for call in app._handle["calls"] if call[:2] == ("after", "cancel")]
timer_token.deletetimerhandler()
timer_token.deletetimerhandler()
cancel_calls_after = [call for call in app._handle["calls"] if call[:2] == ("after", "cancel")]
checks["createtimerhandler_delete_idempotent"] = cancel_calls_after == cancel_calls_before

class _ProbeFile:
    def __init__(self, fd):
        self._fd = int(fd)

    def fileno(self):
        return self._fd

probe_file = _ProbeFile(41)
file_events = []
app.createfilehandler(
    probe_file,
    _tkinter.READABLE | _tkinter.WRITABLE,
    lambda file_obj, mask: file_events.append((file_obj.fileno(), mask)),
)
_dispatch_fileevent(app._handle, probe_file, "readable")
_dispatch_fileevent(app._handle, probe_file, "writable")
readable_command = "::__molt_filehandler_41_readable"
writable_command = "::__molt_filehandler_41_writable"
checks["createfilehandler_registers_runtime_events"] = (
    app._handle["fileevents"].get(41, {}).get("readable") == readable_command
    and app._handle["fileevents"].get(41, {}).get("writable") == writable_command
)
checks["createfilehandler_dispatches_callback_with_file_and_mask"] = (
    file_events == [(41, _tkinter.READABLE), (41, _tkinter.WRITABLE)]
)
try:
    app.createfilehandler(probe_file, _tkinter.READABLE, None)
except BaseException as exc:  # noqa: BLE001
    checks["createfilehandler_bad_callback_error_shape"] = (
        type(exc).__name__ == "TypeError" and str(exc) == "bad argument list"
    )
else:
    checks["createfilehandler_bad_callback_error_shape"] = False
app.deletefilehandler(probe_file)
checks["deletefilehandler_unregisters_runtime_events"] = (
    41 not in app._handle["fileevents"]
    and readable_command not in app._handle["commands"]
    and writable_command not in app._handle["commands"]
)
checks["deletefilehandler_stops_dispatch"] = (
    _dispatch_fileevent(app._handle, probe_file, "readable") is False
)
try:
    app.deletefilehandler(probe_file)
except BaseException:  # noqa: BLE001
    checks["deletefilehandler_idempotent"] = False
else:
    checks["deletefilehandler_idempotent"] = True
trace_marker = object()
app.settrace(trace_marker)
checks["trace_roundtrip"] = app.gettrace() is trace_marker
checks["willdispatch_returns_none"] = app.willdispatch() is None
app.adderrorinfo("phase0-extra")
checks["adderrorinfo_sets_errorinfo"] = app.call("set", "errorInfo").endswith("phase0-extra")

root = tkinter.Tk(useTk=False)
checks["tkinter_create_use_tk_false_forwarded"] = (
    root._tk_app._handle["create_options"].get("useTk") is False
)
root.setvar("phase0_root", "root-value")
checks["tkinter_set_get_roundtrip"] = root.getvar("phase0_root") == "root-value"
checks["tkinter_call_roundtrip"] = (
    root.call("set", "phase0_call", "call-value") == "call-value"
    and root.call("set", "phase0_call") == "call-value"
)
root.createcommand("phase0_root_cmd", lambda: None)
root.deletecommand("phase0_root_cmd")
checks["tkinter_deletecommand_runtime_notified"] = (
    ("rename", "phase0_root_cmd", "") in root._tk_app._handle["calls"]
)
tk_after_events = []
tk_after_token = root.after(9, lambda: tk_after_events.append("root-fired"))
checks["tkinter_after_callback_invoked"] = (
    tk_after_token == "after#9" and tk_after_events == ["root-fired"]
)
protocol_id = root.protocol("WM_DELETE_WINDOW", lambda: None)
protocol_query_before = root.protocol("WM_DELETE_WINDOW")
root.protocol("WM_DELETE_WINDOW", "")
protocol_query_after = root.protocol("WM_DELETE_WINDOW")
checks["tkinter_protocol_registration_roundtrip"] = (
    isinstance(protocol_id, str)
    and protocol_query_before == protocol_id
    and protocol_query_after == ""
    and root._tk_app._handle["wm_protocols"].get((root._w, "WM_DELETE_WINDOW")) is None
)

widget_call_start = len(root._tk_app._handle["calls"])
button = tkinter.Button(root, text="button-probe")
frame = tkinter.Frame(root)
label = tkinter.Label(frame, text="label-probe")
message = tkinter.Message(root, text="message-probe")
widget_calls = root._tk_app._handle["calls"][widget_call_start:]
checks["tkinter_widget_wrappers_emit_commands"] = (
    ("button", button._w, "-text", "button-probe") in widget_calls
    and ("frame", frame._w) in widget_calls
    and ("label", label._w, "-text", "label-probe") in widget_calls
    and ("message", message._w, "-text", "message-probe") in widget_calls
)
checks["tkinter_widget_wrapper_paths_deterministic"] = (
    button._w == ".!button1"
    and frame._w == ".!frame2"
    and label._w == ".!label3"
    and message._w == ".!message4"
)

callback_option_call_start = len(root._tk_app._handle["calls"])
button_with_command = tkinter.Button(root, text="button-command", command=lambda: None)
canvas_with_scroll = tkinter.Canvas(root, yscrollcommand=lambda first, last: None)
scrollbar_with_command = tkinter.Scrollbar(root, command=lambda *args: None)
menu = tkinter.Menu(root)
menu.add_command(label="Refresh", command=lambda: None)
callback_option_calls = root._tk_app._handle["calls"][callback_option_call_start:]

def _command_option_value(call, option_name):
    values = list(call)
    for index, value in enumerate(values[:-1]):
        if value == option_name:
            return values[index + 1]
    return None

button_command_name = _command_option_value(callback_option_calls[0], "-command")
canvas_scroll_name = _command_option_value(callback_option_calls[1], "-yscrollcommand")
scrollbar_command_name = _command_option_value(callback_option_calls[2], "-command")
menu_command_name = _command_option_value(callback_option_calls[4], "-command")

checks["tkinter_callable_widget_options_register_commands"] = (
    isinstance(button_command_name, str)
    and isinstance(canvas_scroll_name, str)
    and isinstance(scrollbar_command_name, str)
    and isinstance(menu_command_name, str)
    and button_command_name in root._tk_app._handle["commands"]
    and canvas_scroll_name in root._tk_app._handle["commands"]
    and scrollbar_command_name in root._tk_app._handle["commands"]
    and menu_command_name in root._tk_app._handle["commands"]
    and button_command_name != str(button_with_command)
    and canvas_scroll_name != str(canvas_with_scroll)
    and scrollbar_command_name != str(scrollbar_with_command)
)

var0 = tkinter.StringVar(root, value="string-probe")
var1 = tkinter.IntVar(root, value="12")
var2 = tkinter.DoubleVar(root, value="2.5")
var3 = tkinter.BooleanVar(root, value="1")
named = tkinter.Variable(root, value="named-probe", name="phase0_named_var")
checks["tkinter_variable_names_deterministic"] = (
    str(var0) == "PY_VAR0"
    and str(var1) == "PY_VAR1"
    and str(var2) == "PY_VAR2"
    and str(var3) == "PY_VAR3"
)
checks["tkinter_variable_roundtrip"] = (
    var0.get() == "string-probe"
    and var1.get() == 12
    and var2.get() == 2.5
    and var3.get() is True
    and named.get() == "named-probe"
)
var0.set("updated")
checks["tkinter_variable_set_uses_tk_call"] = (
    root.getvar("PY_VAR0") == "updated"
    and ("set", "PY_VAR0", "updated") in root._tk_app._handle["calls"]
)
checks["tkinter_named_variable_uses_explicit_name"] = (
    str(named) == "phase0_named_var"
    and root.getvar("phase0_named_var") == "named-probe"
)

simpledialog_string = simpledialog.askstring(
    "Probe",
    "Enter value",
    parent=root,
    initialvalue="alpha",
)
simpledialog_int = simpledialog.askinteger(
    "Probe",
    "Enter integer",
    parent=root,
    initialvalue="7",
    minvalue=1,
    maxvalue=9,
)
simpledialog_float = simpledialog.askfloat(
    "Probe",
    "Enter float",
    parent=root,
    initialvalue="2.5",
    minvalue=1.0,
    maxvalue=3.0,
)
simpledialog_int_bad = simpledialog.askinteger(
    "Probe",
    "Enter integer",
    parent=root,
    initialvalue="not-an-int",
)
checks["tkinter_simpledialog_query_helpers"] = (
    simpledialog_string == "alpha"
    and simpledialog_int == 7
    and simpledialog_float == 2.5
    and simpledialog_int_bad is None
)

checks["tkinter_dialog_alias_exports_present"] = (
    colorchooser.Dialog is commondialog.Dialog
    and filedialog.Dialog is dialog_module.Dialog
    and messagebox.Dialog is commondialog.Dialog
    and simpledialog.messagebox is messagebox
)
checks["tkinter_simpledialog_phase1_exports_present"] = (
    callable(getattr(simpledialog, "_place_window", None))
    and callable(getattr(simpledialog, "_setup_dialog", None))
    and {"_place_window", "_setup_dialog"}.issubset(set(simpledialog.__all__))
)
checks["tkinter_dialog_module_compat_symbols_present"] = (
    dialog_module.DIALOG_ICON == "questhead"
    and dialog_module.TclError is tkinter.TclError
    and dialog_module.Widget is tkinter.Widget
    and dialog_module.Button is tkinter.Button
)
checks["tkinter_filedialog_compat_symbols_present"] = (
    hasattr(filedialog, "FileDialog")
    and hasattr(filedialog, "LoadFileDialog")
    and hasattr(filedialog, "SaveFileDialog")
    and isinstance(filedialog.dialogstates, dict)
    and filedialog.commondialog is commondialog
)

orig_askopenfilename = filedialog.askopenfilename
orig_asksaveasfilename = filedialog.asksaveasfilename
compat_open_calls = []
compat_save_calls = []

def _compat_askopenfilename(**options):
    compat_open_calls.append(dict(options))
    return "/virtual/data/input.txt"

def _compat_asksaveasfilename(**options):
    compat_save_calls.append(dict(options))
    return "/virtual/data/output.txt"

filedialog.askopenfilename = _compat_askopenfilename
filedialog.asksaveasfilename = _compat_asksaveasfilename
try:
    filedialog.dialogstates.clear()
    compat_open_result = filedialog.FileDialog(root, title="Compat Open").go(
        dir_or_file="/virtual/data",
        pattern="*.txt",
        default="seed.txt",
        key="phase0-key",
    )
    compat_load_result = filedialog.LoadFileDialog(root).go(
        dir_or_file="/virtual/data",
        pattern="*.txt",
        default="seed.txt",
    )
    compat_save_result = filedialog.SaveFileDialog(root).go(
        dir_or_file="/virtual/data",
        default="seed-out.txt",
    )
finally:
    filedialog.askopenfilename = orig_askopenfilename
    filedialog.asksaveasfilename = orig_asksaveasfilename

checks["tkinter_filedialog_compat_classes_route_to_wrappers"] = (
    compat_open_result == "/virtual/data/input.txt"
    and compat_load_result == "/virtual/data/input.txt"
    and compat_save_result == "/virtual/data/output.txt"
    and len(compat_open_calls) == 2
    and len(compat_save_calls) == 1
    and compat_open_calls[0].get("parent") is root
    and compat_open_calls[0].get("initialdir") == "/virtual/data"
    and compat_open_calls[0].get("initialfile") == "seed.txt"
    and compat_save_calls[0].get("parent") is root
    and filedialog.dialogstates.get("phase0-key") == ("/virtual/data", "*.txt")
)
checks["tkinter_filedialog_compat_cancel_returns_none"] = (
    filedialog.FileDialog(root).go() is None
)

commondialog_call_start = len(root._tk_app._handle["calls"])
messagebox_result = messagebox.showinfo("Probe", "Message")
askopenfilename_result = filedialog.askopenfilename()
commondialog_calls = root._tk_app._handle["calls"][commondialog_call_start:]

def _commondialog_call_has_parent(command_name):
    for call in commondialog_calls:
        if not call or call[0] != command_name:
            continue
        idx = 1
        while idx + 1 < len(call):
            if str(call[idx]).lower() == "-parent":
                return True
            idx += 2
    return False

checks["tkinter_commondialog_supported_commands_dispatch_to_tk_call"] = (
    messagebox_result == "ok"
    and askopenfilename_result == ""
    and _commondialog_call_has_parent("tk_messageBox")
    and _commondialog_call_has_parent("tk_getOpenFile")
)

class _UnsupportedDialog(commondialog.Dialog):
    command = "tk_chooseFont"

try:
    _UnsupportedDialog(master=root).show()
except BaseException as exc:  # noqa: BLE001
    checks["tkinter_commondialog_unsupported_command_error_shape"] = (
        type(exc).__name__ == "RuntimeError"
        and str(exc) == 'unsupported commondialog command "tk_chooseFont"'
    )
else:
    checks["tkinter_commondialog_unsupported_command_error_shape"] = False

expected_all = {
    "BooleanVar",
    "Button",
    "Canvas",
    "Checkbutton",
    "DoubleVar",
    "Entry",
    "Frame",
    "IntVar",
    "Label",
    "LabelFrame",
    "Listbox",
    "Menu",
    "Message",
    "PanedWindow",
    "Radiobutton",
    "Scale",
    "Scrollbar",
    "Spinbox",
    "StringVar",
    "Text",
    "Toplevel",
    "Variable",
}
checks["tkinter_all_exports_include_widget_and_var_wrappers"] = expected_all.issubset(
    set(tkinter.__all__)
)

ttk_expected_surfaces = {
    "Button",
    "Checkbutton",
    "Combobox",
    "LabelFrame",
    "LabeledScale",
    "Labelframe",
    "Menubutton",
    "Notebook",
    "OptionMenu",
    "PanedWindow",
    "Panedwindow",
    "Progressbar",
    "Scale",
    "Scrollbar",
    "Sizegrip",
    "Spinbox",
    "Style",
    "Treeview",
    "setup_master",
    "tclobjs_to_py",
}
checks["ttk_new_surfaces_exported"] = all(
    hasattr(ttk, name) for name in ttk_expected_surfaces
)
checks["ttk_all_includes_new_surfaces"] = ttk_expected_surfaces.issubset(
    set(ttk.__all__)
)
checks["ttk_alias_exports_present"] = (
    ttk.LabelFrame is ttk.Labelframe and ttk.PanedWindow is ttk.Panedwindow
)
checks["ttk_tkinter_symbol_exported"] = ttk.tkinter is tkinter
checks["ttk_setup_master_default_root"] = (
    ttk.setup_master(root) is root and ttk.setup_master() is root
)
tclobj_probe = {"numbers": ("7", "2.5"), "empty": (), "plain": "ok"}
tclobj_converted = ttk.tclobjs_to_py(tclobj_probe)
checks["ttk_tclobjs_to_py_converts_tuple_like_values"] = (
    tclobj_converted is tclobj_probe
    and tclobj_probe["numbers"] == [7, 2.5]
    and tclobj_probe["empty"] == ""
    and tclobj_probe["plain"] == "ok"
)

ttk_call_start = len(root._tk_app._handle["calls"])
ttk_specs = [
    ("ttk::combobox", lambda: ttk.Combobox(root)),
    ("ttk::treeview", lambda: ttk.Treeview(root)),
    ("ttk::progressbar", lambda: ttk.Progressbar(root)),
    ("ttk::scrollbar", lambda: ttk.Scrollbar(root)),
    ("ttk::spinbox", lambda: ttk.Spinbox(root)),
    ("ttk::labelframe", lambda: ttk.Labelframe(root)),
    ("ttk::menubutton", lambda: ttk.Menubutton(root)),
    (
        "ttk::menubutton",
        lambda: ttk.OptionMenu(root, "phase0_option_var", "default", "other"),
    ),
    ("ttk::sizegrip", lambda: ttk.Sizegrip(root)),
    ("ttk::panedwindow", lambda: ttk.Panedwindow(root)),
]
ttk_widgets = [build() for _, build in ttk_specs]
ttk_calls = root._tk_app._handle["calls"][ttk_call_start:]
ttk_commands = [
    call[0] for call in ttk_calls if call and str(call[0]).startswith("ttk::")
]
checks["ttk_constructor_command_routing"] = ttk_commands == [
    name for name, _ in ttk_specs
]
checks["ttk_widget_paths_initialized"] = all(
    str(widget).startswith(".!") for widget in ttk_widgets
)

ttk_method_start = len(root._tk_app._handle["calls"])
ttk_button = ttk.Button(root, text="ttk-button")
ttk_checkbutton = ttk.Checkbutton(root, text="ttk-check")
ttk_entry = ttk.Entry(root)
ttk_combobox = ttk.Combobox(root)
ttk_notebook = ttk.Notebook(root)
ttk_tab_frame = ttk.Frame(ttk_notebook)
ttk_paned = ttk.Panedwindow(root)
ttk_progress = ttk.Progressbar(root)
ttk_radio = ttk.Radiobutton(root, text="ttk-radio")
ttk_scale = ttk.Scale(root)
ttk_spinbox = ttk.Spinbox(root)
ttk_tree = ttk.Treeview(root)
ttk_labeled = ttk.LabeledScale(root, from_=1, to=9)
ttk_option_var = tkinter.StringVar(root, value="default")
ttk_option = ttk.OptionMenu(root, ttk_option_var, "default", "alpha")

ttk_button.invoke()
ttk_checkbutton.invoke()
ttk_entry.bbox(0)
ttk_entry.identify(4, 5)
ttk_entry.validate()
ttk_combobox.current()
ttk_combobox.current(2)
ttk_combobox.set("combo-value")
ttk_notebook.add(ttk_tab_frame, text="tab-a")
ttk_notebook.hide(ttk_tab_frame)
ttk_notebook.forget(ttk_tab_frame)
ttk_notebook.identify(2, 3)
ttk_notebook.index("end")
ttk_notebook.insert("end", ttk_tab_frame, text="tab-b")
ttk_notebook.select()
ttk_notebook.select(ttk_tab_frame)
ttk_notebook.tab(ttk_tab_frame, text="tab-c")
ttk_notebook.tab(ttk_tab_frame, option="text")
ttk_notebook.tabs()
ttk_notebook.enable_traversal()
ttk_paned.insert("end", ttk_tab_frame, weight=1)
ttk_paned.pane(ttk_tab_frame, weight=2)
ttk_paned.pane(ttk_tab_frame, option="weight")
ttk_paned.forget(ttk_tab_frame)
ttk_paned.sashpos(0)
ttk_paned.sashpos(0, 9)
ttk_progress.start()
ttk_progress.start(17)
ttk_progress.step()
ttk_progress.step(3)
ttk_progress.stop()
ttk_radio.invoke()
ttk_scale.get()
ttk_scale.get(6, 7)
ttk_scale.configure({"from": 1, "to": 9})
ttk_scale.configure(from_=2)
ttk_scale.configure(command="noop")
ttk_spinbox.set("spin-value")
ttk_bbox = ttk_tree.bbox("row0")
ttk_children_root = ttk_tree.get_children()
ttk_children_row0 = ttk_tree.get_children("row0")
ttk_tree.set_children("row0", "row1", "row2")
ttk_tree.column("#0", width=120)
ttk_tree.column("#0", option="width")
ttk_tree.delete("row1", "row2")
ttk_tree.detach("row3")
ttk_exists_row3 = ttk_tree.exists("row3")
ttk_tree.focus()
ttk_tree.focus("row4")
ttk_tree.heading("#0", text="Tree")
ttk_tree.heading("#0", option="text")
ttk_tree.identify("row", 1, 2)
ttk_tree.identify_row(3)
ttk_tree.identify_column(4)
ttk_tree.identify_region(5, 6)
ttk_tree.identify_element(7, 8)
ttk_tree_index = ttk_tree.index("row4")
ttk_tree.insert("", "end", iid="row1", text="Row One")
ttk_tree.item("row1", text="Row One Updated")
ttk_tree.item("row1", option="text")
ttk_tree.move("row1", "", 0)
ttk_tree.next("row1")
ttk_tree.parent("row1")
ttk_tree.prev("row1")
ttk_tree.see("row1")
ttk_selection = ttk_tree.selection()
ttk_tree.selection_set("row1")
ttk_tree.selection_add("row2")
ttk_tree.selection_remove("row1")
ttk_tree.selection_toggle("row1", "row2")
ttk_set_all = ttk_tree.set("row1")
ttk_tree.set("row1", "#0")
ttk_tree.set("row1", "#0", "cell-value")
ttk_tag_select_query = ttk_tree.tag_bind("tag1", "<<TreeviewSelect>>")
ttk_tag_events = []
ttk_tag_event_payload = []

def _on_tree_tag_event(event):
    ttk_tag_events.append(getattr(event, "widget", None) is ttk_tree)
    ttk_tag_event_payload.append(
        (
            getattr(event, "x", None),
            getattr(event, "y", None),
            getattr(event, "delta", None),
        )
    )

ttk_tag_funcid = ttk_tree.tag_bind("tag1", "<<TreeviewOpen>>", _on_tree_tag_event)
ttk_tag_open_query_before = ttk_tree.tag_bind("tag1", "<<TreeviewOpen>>")
if ttk_tag_funcid in root._tk_app._handle["commands"]:
    root._tk_app._handle["commands"][ttk_tag_funcid]()
if ttk_tag_funcid in root._tk_app._handle["commands"]:
    root._tk_app._handle["commands"][ttk_tag_funcid](object())
if ttk_tag_funcid in root._tk_app._handle["commands"]:
    root._tk_app._handle["commands"][ttk_tag_funcid](
        "7",
        "1",
        "1",
        "10",
        "11",
        "12",
        "13",
        "14",
        "15",
        "16",
        "A",
        "1",
        "K",
        "65",
        ttk_tree._w,
        "VirtualEvent",
        "115",
        "116",
        "120",
    )
ttk_tree.tag_unbind("tag1", "<<TreeviewOpen>>", ttk_tag_funcid)
ttk_tag_open_query_after = ttk_tree.tag_bind("tag1", "<<TreeviewOpen>>")
ttk_tree.tag_configure("tag1", foreground="blue")
ttk_tree.tag_configure("tag1", option="foreground")
ttk_tag_has_all = ttk_tree.tag_has("tag1")
ttk_tag_has_row1 = ttk_tree.tag_has("tag1", "row1")
ttk_option.set_menu("beta", "beta", "gamma")
labeled_has_children_before_destroy = isinstance(ttk_labeled.label, ttk.Label) and isinstance(
    ttk_labeled.scale, ttk.Scale
)
labeled_value_before = ttk_labeled.value
ttk_labeled.value = 4
labeled_value_after = ttk_labeled.value
ttk_labeled.destroy()
labeled_destroyed = ttk_labeled.label is None and ttk_labeled.scale is None
optionmenu_had_variable = hasattr(ttk_option, "_variable")
ttk_option.destroy()

style = ttk.Style(root)
style.configure("Probe.TButton", padding=4)
style.configure("Probe.TButton", query_opt="padding")
style.map("Probe.TButton", foreground=[("active", "blue")])
style.map("Probe.TButton", query_opt="foreground")
style.lookup("Probe.TButton", "foreground", ("active",), "fallback")
style.layout("Probe.TButton")
style.layout("Probe.TButton", [("Button.border", {"sticky": "nswe"})])
style.element_create("Probe.indicator", "from", "default", "Button.button")
style.element_names()
style.element_options("Probe.indicator")
style.theme_create(
    "probe_theme",
    parent="default",
    settings={"Probe.TButton": {"configure": {"padding": 1}}},
)
style.theme_settings(
    "probe_theme",
    {"Probe.TButton": {"map": {"foreground": [("active", "blue")]}}},
)
style.theme_names()
style.theme_use("probe_theme")
style.theme_use()

try:
    style.configure("Probe.TButton", query_opt="padding", padding=8)
except BaseException as exc:  # noqa: BLE001
    checks["ttk_style_configure_conflict_error"] = (
        type(exc).__name__ == "TypeError"
        and "cannot be combined with update options" in str(exc)
    )
else:
    checks["ttk_style_configure_conflict_error"] = False

try:
    style.map("Probe.TButton", query_opt="foreground", foreground=[])
except BaseException as exc:  # noqa: BLE001
    checks["ttk_style_map_conflict_error"] = (
        type(exc).__name__ == "TypeError"
        and "cannot be combined with update options" in str(exc)
    )
else:
    checks["ttk_style_map_conflict_error"] = False

try:
    ttk_notebook.tab(ttk_tab_frame, option="text", text="conflict")
except BaseException as exc:  # noqa: BLE001
    checks["ttk_notebook_tab_conflict_error"] = (
        type(exc).__name__ == "TypeError"
        and "cannot be combined with update options" in str(exc)
    )
else:
    checks["ttk_notebook_tab_conflict_error"] = False

try:
    ttk_tree.column("#0", option="width", width=100)
except BaseException as exc:  # noqa: BLE001
    checks["ttk_tree_column_conflict_error"] = (
        type(exc).__name__ == "TypeError"
        and "cannot be combined with update options" in str(exc)
    )
else:
    checks["ttk_tree_column_conflict_error"] = False

checks["ttk_treeview_return_fidelity"] = (
    isinstance(ttk_bbox, tuple)
    and ttk_bbox == (1, 2, 3, 4)
    and isinstance(ttk_children_root, tuple)
    and isinstance(ttk_children_row0, tuple)
    and isinstance(ttk_exists_row3, bool)
    and ttk_exists_row3 is True
    and isinstance(ttk_tree_index, int)
    and ttk_tree_index == 7
    and isinstance(ttk_selection, tuple)
    and isinstance(ttk_set_all, dict)
    and ttk_set_all.get("#0") == "row-text"
    and ttk_set_all.get("value") == "row-value"
    and isinstance(ttk_tag_has_all, tuple)
    and isinstance(ttk_tag_has_row1, bool)
    and ttk_tag_has_row1 is True
)

checks["ttk_treeview_tag_bind_callback_fidelity"] = (
    isinstance(ttk_tag_funcid, str)
    and isinstance(ttk_tag_open_query_before, str)
    and ttk_tag_funcid in ttk_tag_open_query_before
    and ttk_tag_open_query_after == ""
    and ttk_tag_select_query == ""
    and ttk_tag_funcid not in root._tk_app._handle["commands"]
    and ttk_tag_events == [True, False, True]
    and ttk_tag_event_payload[-1] == (15, 16, 120)
)

method_calls = root._tk_app._handle["calls"][ttk_method_start:]

def _saw_prefix(*expected):
    n = len(expected)
    return any(tuple(call[:n]) == expected for call in method_calls)

checks["ttk_widget_common_methods_forwarded"] = (
    _saw_prefix(ttk_button._w, "invoke")
    and _saw_prefix(ttk_checkbutton._w, "invoke")
    and _saw_prefix(ttk_entry._w, "bbox", 0)
    and _saw_prefix(ttk_entry._w, "identify", 4, 5)
    and _saw_prefix(ttk_entry._w, "validate")
    and _saw_prefix(ttk_combobox._w, "current")
    and _saw_prefix(ttk_combobox._w, "current", 2)
    and _saw_prefix(ttk_combobox._w, "set", "combo-value")
    and _saw_prefix(ttk_progress._w, "start")
    and _saw_prefix(ttk_progress._w, "start", 17)
    and _saw_prefix(ttk_progress._w, "step")
    and _saw_prefix(ttk_progress._w, "step", 3)
    and _saw_prefix(ttk_progress._w, "stop")
    and _saw_prefix(ttk_radio._w, "invoke")
    and _saw_prefix(ttk_scale._w, "get")
    and _saw_prefix(ttk_scale._w, "get", 6, 7)
    and _saw_prefix(ttk_spinbox._w, "set", "spin-value")
)

checks["ttk_notebook_methods_forwarded"] = (
    _saw_prefix(ttk_notebook._w, "add", ttk_tab_frame, "-text", "tab-a")
    and _saw_prefix(ttk_notebook._w, "hide", ttk_tab_frame)
    and _saw_prefix(ttk_notebook._w, "forget", ttk_tab_frame)
    and _saw_prefix(ttk_notebook._w, "identify", 2, 3)
    and _saw_prefix(ttk_notebook._w, "index", "end")
    and _saw_prefix(ttk_notebook._w, "insert", "end", ttk_tab_frame, "-text", "tab-b")
    and _saw_prefix(ttk_notebook._w, "select")
    and _saw_prefix(ttk_notebook._w, "select", ttk_tab_frame)
    and _saw_prefix(ttk_notebook._w, "tab", ttk_tab_frame, "-text", "tab-c")
    and _saw_prefix(ttk_notebook._w, "tab", ttk_tab_frame, "-text")
    and _saw_prefix(ttk_notebook._w, "tabs")
    and _saw_prefix("ttk::notebook::enableTraversal", ttk_notebook._w)
)

checks["ttk_panedwindow_methods_forwarded"] = (
    _saw_prefix(ttk_paned._w, "insert", "end", ttk_tab_frame, "-weight", 1)
    and _saw_prefix(ttk_paned._w, "pane", ttk_tab_frame, "-weight", 2)
    and _saw_prefix(ttk_paned._w, "pane", ttk_tab_frame, "-weight")
    and _saw_prefix(ttk_paned._w, "forget", ttk_tab_frame)
    and _saw_prefix(ttk_paned._w, "sashpos", 0)
    and _saw_prefix(ttk_paned._w, "sashpos", 0, 9)
)

range_changed_events = [
    call
    for call in method_calls
    if tuple(call[:4]) == ("event", "generate", ttk_scale._w, "<<RangeChanged>>")
]
checks["ttk_scale_configure_emits_range_changed_event"] = (
    _saw_prefix(ttk_scale._w, "configure", "-from", 1, "-to", 9)
    and _saw_prefix(ttk_scale._w, "configure", "-from_", 2)
    and _saw_prefix(ttk_scale._w, "configure", "-command", "noop")
    and len(range_changed_events) == 2
)

checks["ttk_treeview_methods_forwarded"] = (
    _saw_prefix(ttk_tree._w, "bbox", "row0")
    and _saw_prefix(ttk_tree._w, "children", "")
    and _saw_prefix(ttk_tree._w, "children", "row0")
    and _saw_prefix(ttk_tree._w, "children", "row0", ("row1", "row2"))
    and _saw_prefix(ttk_tree._w, "column", "#0", "-width", 120)
    and _saw_prefix(ttk_tree._w, "column", "#0", "-width")
    and _saw_prefix(ttk_tree._w, "delete", "row1", "row2")
    and _saw_prefix(ttk_tree._w, "detach", "row3")
    and _saw_prefix(ttk_tree._w, "exists", "row3")
    and _saw_prefix(ttk_tree._w, "focus")
    and _saw_prefix(ttk_tree._w, "focus", "row4")
    and _saw_prefix(ttk_tree._w, "heading", "#0", "-text", "Tree")
    and _saw_prefix(ttk_tree._w, "heading", "#0", "-text")
    and _saw_prefix(ttk_tree._w, "identify", "row", 1, 2)
    and _saw_prefix(ttk_tree._w, "identify", "row", 0, 3)
    and _saw_prefix(ttk_tree._w, "identify", "column", 4, 0)
    and _saw_prefix(ttk_tree._w, "identify", "region", 5, 6)
    and _saw_prefix(ttk_tree._w, "identify", "element", 7, 8)
    and _saw_prefix(ttk_tree._w, "index", "row4")
    and _saw_prefix(ttk_tree._w, "insert", "", "end", "-id", "row1", "-text", "Row One")
    and _saw_prefix(ttk_tree._w, "item", "row1", "-text", "Row One Updated")
    and _saw_prefix(ttk_tree._w, "item", "row1", "-text")
    and _saw_prefix(ttk_tree._w, "move", "row1", "", 0)
    and _saw_prefix(ttk_tree._w, "next", "row1")
    and _saw_prefix(ttk_tree._w, "parent", "row1")
    and _saw_prefix(ttk_tree._w, "prev", "row1")
    and _saw_prefix(ttk_tree._w, "see", "row1")
    and _saw_prefix(ttk_tree._w, "selection")
    and _saw_prefix(ttk_tree._w, "selection", "set", "row1")
    and _saw_prefix(ttk_tree._w, "selection", "add", "row2")
    and _saw_prefix(ttk_tree._w, "selection", "remove", "row1")
    and _saw_prefix(ttk_tree._w, "selection", "toggle", "row1", "row2")
    and _saw_prefix(ttk_tree._w, "set", "row1")
    and _saw_prefix(ttk_tree._w, "set", "row1", "#0")
    and _saw_prefix(ttk_tree._w, "set", "row1", "#0", "cell-value")
    and _saw_prefix(ttk_tree._w, "tag", "bind", "tag1", "<<TreeviewSelect>>")
    and _saw_prefix(ttk_tree._w, "tag", "bind", "tag1", "<<TreeviewOpen>>")
    and _saw_prefix(ttk_tree._w, "tag", "configure", "tag1", "-foreground", "blue")
    and _saw_prefix(ttk_tree._w, "tag", "configure", "tag1", "-foreground")
    and _saw_prefix(ttk_tree._w, "tag", "has", "tag1")
    and _saw_prefix(ttk_tree._w, "tag", "has", "tag1", "row1")
)

checks["ttk_style_methods_forwarded"] = (
    _saw_prefix("ttk::style", "configure", "Probe.TButton")
    and _saw_prefix("ttk::style", "map", "Probe.TButton")
    and _saw_prefix("ttk::style", "lookup", "Probe.TButton")
    and _saw_prefix("ttk::style", "layout", "Probe.TButton")
    and _saw_prefix("ttk::style", "element", "create", "Probe.indicator")
    and _saw_prefix("ttk::style", "element", "names")
    and _saw_prefix("ttk::style", "element", "options", "Probe.indicator")
    and _saw_prefix("ttk::style", "theme", "create", "probe_theme")
    and _saw_prefix("ttk::style", "theme", "settings", "probe_theme")
    and _saw_prefix("ttk::style", "theme", "names")
    and _saw_prefix("ttk::style", "theme", "use", "probe_theme")
    and _saw_prefix("ttk::style", "theme", "use")
)

checks["ttk_optionmenu_set_menu_updates_values"] = (
    ttk_option.default == "beta"
    and ttk_option.values == ("beta", "gamma")
    and ttk_option_var.get() == "beta"
)
checks["ttk_optionmenu_destroy_releases_variable"] = optionmenu_had_variable and (
    not hasattr(ttk_option, "_variable") and not hasattr(ttk_option, "variable")
)
checks["ttk_labeledscale_wrapper_sanity"] = (
    labeled_has_children_before_destroy
    and labeled_value_before == 1
    and labeled_value_after == 4
    and labeled_destroyed
)
root.destroy()

for key in sorted(checks):
    print(f"CHECK|{key}|{checks[key]}")
"""

_TTK_PHASE0_PARITY_PROBE = """
import builtins
import fnmatch as _host_fnmatch
import sys
import types as _host_types

sys.path.insert(0, __STDLIB_ROOT__)

def _capabilities_has(name=None):
    return name in {"gui.window", "gui", "process.spawn", "process"}

def _types_bootstrap():
    keys = (
        "AsyncGeneratorType",
        "BuiltinFunctionType",
        "BuiltinMethodType",
        "CapsuleType",
        "CellType",
        "ClassMethodDescriptorType",
        "CodeType",
        "CoroutineType",
        "EllipsisType",
        "FrameType",
        "FunctionType",
        "GeneratorType",
        "MappingProxyType",
        "MethodType",
        "MethodDescriptorType",
        "MethodWrapperType",
        "ModuleType",
        "NoneType",
        "NotImplementedType",
        "GenericAlias",
        "GetSetDescriptorType",
        "LambdaType",
        "MemberDescriptorType",
        "SimpleNamespace",
        "TracebackType",
        "UnionType",
        "WrapperDescriptorType",
        "DynamicClassAttribute",
        "coroutine",
        "get_original_bases",
        "new_class",
        "prepare_class",
        "resolve_bases",
    )
    return {name: getattr(_host_types, name) for name in keys}

def _app_new(_opts=None):
    create_options = dict(_opts or {})
    return {
        "vars": {},
        "commands": {},
        "tree_tag_bindings": {},
        "calls": [],
        "create_options": create_options,
    }

def _tk_call(app, argv):
    app["calls"].append(tuple(argv))
    if not argv:
        return None
    op = str(argv[0])
    if op == "set":
        if len(argv) == 2:
            return app["vars"][str(argv[1])]
        if len(argv) == 3:
            app["vars"][str(argv[1])] = argv[2]
            return argv[2]
        raise RuntimeError("invalid set arity")
    if op == "unset":
        if len(argv) != 2:
            raise RuntimeError("invalid unset arity")
        app["vars"].pop(str(argv[1]), None)
        return ""
    if op == "rename" and len(argv) == 3 and argv[2] == "":
        app["commands"].pop(str(argv[1]), None)
        return ""
    if len(argv) >= 2 and str(argv[1]) == "selection":
        return ()
    if len(argv) >= 2 and str(argv[1]) == "set":
        if len(argv) == 3:
            return ()
        return ""
    return tuple(argv)

def _tk_bind_command(app, name, callback):
    app["commands"][str(name)] = callback

def _tk_unbind_command(app, name):
    app["commands"].pop(str(name), None)
    app["calls"].append(("rename", str(name), ""))
    return None

def _tk_after(_app, delay_ms, callback):
    callback()
    return f"after#{int(delay_ms)}"

_command_serial = {"value": 0}


def _next_command_name(prefix):
    _command_serial["value"] += 1
    return f"{prefix}{_command_serial['value']}"


def _tk_after_idle(_app, callback):
    callback()
    return "after#idle"


def _tk_after_cancel(_app, _token):
    return None


def _tk_after_info(_app=None, _identifier=None):
    return ()


def _tk_trace_add(app, _name, _mode, callback):
    command_name = _next_command_name("::__molt_trace_")
    app["commands"][command_name] = callback
    return command_name


def _tk_trace_remove(app, _name, _mode, callback_name):
    app["commands"].pop(str(callback_name), None)
    return None


def _tk_trace_clear(_app=None, _name=None):
    return None


def _tk_trace_info(_app, _name):
    return ()


def _tk_tkwait_variable(_app, _name):
    return None


def _tk_tkwait_window(_app, _target):
    return None


def _tk_tkwait_visibility(_app, _target):
    return None


def _tk_bind_callback_register(app, _target_name, _sequence, callback, _add_prefix):
    command_name = _next_command_name("::__molt_bind_")
    app["commands"][command_name] = callback
    return command_name


def _tk_bind_callback_unregister(app, _target_name, _sequence, command_name):
    app["commands"].pop(str(command_name), None)
    return None


def _tk_treeview_tag_bind_callback_register(app, treeview_path, tagname, sequence, callback):
    command_name = _next_command_name("::__molt_ttk_tag_")
    app["commands"][command_name] = callback
    app["tree_tag_bindings"][(str(treeview_path), str(tagname), str(sequence))] = command_name
    return command_name


def _tk_treeview_tag_bind_callback_unregister(
    app, treeview_path, tagname, sequence, command_name
):
    key = (str(treeview_path), str(tagname), str(sequence))
    if app["tree_tag_bindings"].get(key) == str(command_name):
        app["tree_tag_bindings"][key] = ""
    app["commands"].pop(str(command_name), None)
    return None


def _tk_getboolean(value):
    if isinstance(value, str):
        lowered = value.strip().lower()
        if lowered in {"1", "true", "yes", "on"}:
            return True
        if lowered in {"0", "false", "no", "off", ""}:
            return False
    return bool(value)


def _tk_getdouble(value):
    return float(value)


def _tk_splitlist(value):
    if isinstance(value, (tuple, list)):
        return tuple(value)
    text = str(value).strip()
    if not text:
        return ()
    return tuple(text.split())


def _tk_errorinfo_append(app, text):
    current = app["vars"].get("errorInfo")
    if current:
        app["vars"]["errorInfo"] = f"{current}\\n{text}"
    else:
        app["vars"]["errorInfo"] = str(text)
    return None


def _tk_event_subst_parse(_widget_path, event_args):
    return tuple(event_args)


def _tk_convert_stringval(value):
    if isinstance(value, (int, float)) or not isinstance(value, str):
        return value
    text = value.strip()
    if not text:
        return value
    try:
        return int(text, 10)
    except ValueError:
        pass
    try:
        return float(text)
    except ValueError:
        return value


def _tk_hex_to_rgb(color):
    text = str(color).lstrip("#")
    if len(text) != 6:
        return color
    return tuple(int(text[index : index + 2], 16) for index in (0, 2, 4))


def _tk_event_int(value):
    return int(value)


def _tk_event_build_from_args(widget_path, event_args):
    values = list(event_args)
    event = {"widget": widget_path}
    if len(values) > 8:
        event["x"] = int(values[8])
    if len(values) > 9:
        event["y"] = int(values[9])
    if len(values) > 18:
        event["delta"] = int(values[18])
    return event


def _tk_event_state_decode(value):
    return () if int(value) == 0 else (str(int(value)),)


def _tk_splitdict(value, _cut_minus=True):
    if isinstance(value, dict):
        return list(value.items())
    items = list(value)
    return [(items[idx], items[idx + 1]) for idx in range(0, len(items) - 1, 2)]


def _tk_flatten_args(seq):
    out = []
    for item in seq:
        if isinstance(item, (list, tuple)):
            out.extend(_tk_flatten_args(item))
        else:
            out.append(item)
    return out


def _tk_cnfmerge(cnfs, _fallback=None):
    merged = {}
    for cnf in cnfs or ():
        if isinstance(cnf, dict):
            merged.update(cnf)
    return merged


def _tk_normalize_option(name):
    text = str(name)
    if text.endswith("_"):
        text = text[:-1]
    text = text.replace("_", "-")
    return text if text.startswith("-") else f"-{text}"


def _tk_normalize_delay_ms(value):
    return int(value)


builtins._molt_intrinsics = {
    "molt_stdlib_probe": lambda: True,
    "molt_fnmatch": lambda name, pat: _host_fnmatch.fnmatch(name, pat),
    "molt_fnmatchcase": lambda name, pat: _host_fnmatch.fnmatchcase(name, pat),
    "molt_fnmatch_filter": lambda names, pat, _casefold=False: _host_fnmatch.filter(list(names), pat),
    "molt_fnmatch_translate": lambda pat: _host_fnmatch.translate(pat),
    "molt_types_bootstrap": _types_bootstrap,
    "molt_capabilities_has": _capabilities_has,
    "molt_tk_available": lambda: True,
    "molt_tk_app_new": _app_new,
    "molt_tk_quit": lambda _app=None: None,
    "molt_tk_mainloop": lambda _app=None: None,
    "molt_tk_do_one_event": lambda _app=None, _flags=None: False,
    "molt_tk_after": _tk_after,
    "molt_tk_after_idle": _tk_after_idle,
    "molt_tk_after_cancel": _tk_after_cancel,
    "molt_tk_after_info": _tk_after_info,
    "molt_tk_call": _tk_call,
    "molt_tk_trace_add": _tk_trace_add,
    "molt_tk_trace_remove": _tk_trace_remove,
    "molt_tk_trace_clear": _tk_trace_clear,
    "molt_tk_trace_info": _tk_trace_info,
    "molt_tk_tkwait_variable": _tk_tkwait_variable,
    "molt_tk_tkwait_window": _tk_tkwait_window,
    "molt_tk_tkwait_visibility": _tk_tkwait_visibility,
    "molt_tk_bind_callback_register": _tk_bind_callback_register,
    "molt_tk_bind_callback_unregister": _tk_bind_callback_unregister,
    "molt_tk_widget_bind_callback_register": _tk_bind_callback_register,
    "molt_tk_widget_bind_callback_unregister": _tk_bind_callback_unregister,
    "molt_tk_text_tag_bind_callback_register": _tk_bind_callback_register,
    "molt_tk_text_tag_bind_callback_unregister": _tk_bind_callback_unregister,
    "molt_tk_treeview_tag_bind_callback_register": _tk_treeview_tag_bind_callback_register,
    "molt_tk_treeview_tag_bind_callback_unregister": _tk_treeview_tag_bind_callback_unregister,
    "molt_tk_bind_command": _tk_bind_command,
    "molt_tk_unbind_command": _tk_unbind_command,
    "molt_tk_filehandler_create": lambda _app=None, _fd=None, _mask=None, _callback=None, _file=None: None,
    "molt_tk_filehandler_delete": lambda _app=None, _fd=None: None,
    "molt_tk_destroy_widget": lambda _app=None, _w=None: None,
    "molt_tk_last_error": lambda _app=None: None,
    "molt_tk_getboolean": _tk_getboolean,
    "molt_tk_getdouble": _tk_getdouble,
    "molt_tk_splitlist": _tk_splitlist,
    "molt_tk_errorinfo_append": _tk_errorinfo_append,
    "molt_tk_bind_script_remove_command": lambda _script=None, _command_name=None: _script,
    "molt_tk_event_subst_parse": _tk_event_subst_parse,
    "molt_tk_event_int": _tk_event_int,
    "molt_tk_event_build_from_args": _tk_event_build_from_args,
    "molt_tk_event_state_decode": _tk_event_state_decode,
    "molt_tk_splitdict": _tk_splitdict,
    "molt_tk_flatten_args": _tk_flatten_args,
    "molt_tk_cnfmerge": _tk_cnfmerge,
    "molt_tk_normalize_option": _tk_normalize_option,
    "molt_tk_normalize_delay_ms": _tk_normalize_delay_ms,
    "molt_tk_convert_stringval": _tk_convert_stringval,
    "molt_tk_hex_to_rgb": _tk_hex_to_rgb,
    "molt_tk_commondialog_show": lambda _app=None, _master=None, _command=None, _options=None: "",
    "molt_tk_messagebox_show": lambda _app=None, _master=None, _options=None: "",
    "molt_tk_filedialog_show": lambda _app=None, _master=None, _command=None, _options=None: "",
    "molt_tk_dialog_show": lambda _app=None, _master=None, _title=None, _text=None, _bitmap=None, _default=None, _strings=None: 0,
    "molt_tk_simpledialog_query": lambda _app=None, _parent=None, _title=None, _prompt=None, _initial=None, _kind=None, _min=None, _max=None: "",
}

import tkinter
import tkinter.ttk as ttk

checks = {}
root = tkinter.Tk(useTk=False)

checks["exports"] = all(
    hasattr(ttk, name)
    for name in (
        "LabeledScale",
        "OptionMenu",
        "PanedWindow",
        "Panedwindow",
        "Scale",
        "setup_master",
        "tclobjs_to_py",
    )
)
checks["all"] = {"LabeledScale", "setup_master", "tclobjs_to_py"}.issubset(
    set(ttk.__all__)
)
checks["tkinter_symbol"] = ttk.tkinter is tkinter
checks["setup_master"] = ttk.setup_master(root) is root and ttk.setup_master() is root

tclobj_probe = {"numbers": ("7", "2.5"), "empty": (), "plain": "ok"}
tclobj_converted = ttk.tclobjs_to_py(tclobj_probe)
checks["tclobjs_to_py"] = (
    tclobj_converted is tclobj_probe
    and tclobj_probe["numbers"] == [7, 2.5]
    and tclobj_probe["empty"] == ""
    and tclobj_probe["plain"] == "ok"
)

paned = ttk.Panedwindow(root)
pane_child = ttk.Frame(paned)
paned.forget(pane_child)

scale = ttk.Scale(root)
scale.configure({"from": 1, "to": 9})
scale.configure(from_=2)

option_var = tkinter.StringVar(root, value="alpha")
option = ttk.OptionMenu(root, option_var, "alpha", "beta")
had_option_var = hasattr(option, "_variable")
option.destroy()
checks["optionmenu_destroy"] = had_option_var and not hasattr(option, "_variable")

labeled = ttk.LabeledScale(root, from_=1, to=4)
labeled_start = labeled.value
labeled.value = 3
labeled.destroy()
checks["labeledscale"] = (
    labeled_start == 1 and labeled.label is None and labeled.scale is None
)

calls = root._tk_app._handle["calls"]

def _saw_prefix(*expected):
    n = len(expected)
    return any(tuple(call[:n]) == expected for call in calls)

range_changed_events = [
    call
    for call in calls
    if tuple(call[:4]) == ("event", "generate", scale._w, "<<RangeChanged>>")
]
checks["panedwindow_forget"] = _saw_prefix(paned._w, "forget", pane_child)
checks["scale_configure"] = (
    _saw_prefix(scale._w, "configure", "-from", 1, "-to", 9)
    and _saw_prefix(scale._w, "configure", "-from_", 2)
    and len(range_changed_events) == 2
)

root.destroy()

for key in sorted(checks):
    print(f"CHECK|{key}|{checks[key]}")
"""


def test_tkinter_phase0_wrappers_keep_deterministic_error_contracts() -> None:
    lines = _run_probe(_UNAVAILABLE_RUNTIME_PROBE)

    import_errors = [
        line for line in lines if line.startswith("IMPORT|") and "|error|" in line
    ]
    assert not import_errors, f"import failures: {import_errors}"

    call_failures = [
        line for line in lines if line.startswith("CALL|") and "|error|" in line
    ]

    assert call_failures, "expected Tk-unavailable failures for wrapper calls"

    unexpected_error_types = [
        line
        for line in call_failures
        if "|RuntimeError|" not in line and "|PermissionError|" not in line
    ]
    assert not unexpected_error_types, (
        f"unexpected call error type(s): {unexpected_error_types}"
    )

    tk_available_calls: dict[str, int] = {}
    for line in lines:
        if not line.startswith("TK_AVAILABLE_CALLS|"):
            continue
        _, label, raw_count = line.split("|", 2)
        tk_available_calls[label] = int(raw_count)

    for label in ("main", "dnd_start"):
        assert tk_available_calls.get(label, 0) >= 1, (
            f"expected {label} to query molt_tk_available at least once: "
            f"{tk_available_calls}"
        )

    checks: dict[str, bool] = {}
    for line in lines:
        if not line.startswith("CHECK|"):
            continue
        _, key, raw = line.split("|", 2)
        checks[key] = raw == "True"
    expected_checks = {"_tkinter_tk_available_false", "tkinter_tk_available_false"}
    missing_checks = sorted(expected_checks - checks.keys())
    assert not missing_checks, (
        f"missing expected unavailable-lane checks: {missing_checks}"
    )
    failed_checks = sorted(name for name in expected_checks if not checks[name])
    assert not failed_checks, f"unavailable-lane checks failed: {failed_checks}"

    gui_gate_lines = [line for line in lines if line.startswith("GATE|gui|")]
    process_gate_lines = [line for line in lines if line.startswith("GATE|process|")]
    assert gui_gate_lines and process_gate_lines, (
        "expected explicit capability gate probes"
    )

    for line in gui_gate_lines + process_gate_lines:
        assert "|error|PermissionError|" in line, (
            f"expected deterministic permission error from capability gate: {line}"
        )
    assert any("gui.window" in line for line in gui_gate_lines)
    assert any("process.spawn" in line for line in process_gate_lines)

    tk_ctor_lines = [
        line
        for line in lines
        if line.startswith("UNAVAILABLE|tk_constructor_use_tk_false|")
    ]
    assert len(tk_ctor_lines) == 1, (
        f"expected one unavailable-lane Tk constructor probe, saw: {tk_ctor_lines}"
    )
    assert "|error|RuntimeError|" in tk_ctor_lines[0], (
        "expected deterministic RuntimeError when Tk runtime is unavailable"
    )
    assert "tk operation unavailable" in tk_ctor_lines[0]


def test_tkinter_phase0_wrappers_support_headless_intrinsic_stubs() -> None:
    lines = _run_probe(_HEADLESS_RUNTIME_PROBE)
    checks: dict[str, bool] = {}
    for line in lines:
        if not line.startswith("CHECK|"):
            continue
        _, key, raw = line.split("|", 2)
        checks[key] = raw == "True"

    expected = {
        "after_callback_invoked",
        "adderrorinfo_sets_errorinfo",
        "create_use_tk_false_forwarded",
        "createfilehandler_bad_callback_error_shape",
        "createfilehandler_dispatches_callback_with_file_and_mask",
        "createfilehandler_registers_runtime_events",
        "createtimerhandler_callback_invoked",
        "createtimerhandler_delete_idempotent",
        "createtimerhandler_repr_marks_deleted",
        "createtimerhandler_returns_tktt",
        "deletefilehandler_idempotent",
        "deletefilehandler_stops_dispatch",
        "deletefilehandler_unregisters_runtime_events",
        "deletecommand_runtime_notified",
        "last_error_passthrough",
        "set_get_roundtrip",
        "tk_available",
        "tkinter_all_exports_include_widget_and_var_wrappers",
        "tkinter_after_callback_invoked",
        "tkinter_call_roundtrip",
        "tkinter_callable_widget_options_register_commands",
        "tkinter_commondialog_supported_commands_dispatch_to_tk_call",
        "tkinter_commondialog_unsupported_command_error_shape",
        "tkinter_create_use_tk_false_forwarded",
        "tkinter_deletecommand_runtime_notified",
        "tkinter_dialog_alias_exports_present",
        "tkinter_dialog_module_compat_symbols_present",
        "tkinter_filedialog_compat_cancel_returns_none",
        "tkinter_filedialog_compat_classes_route_to_wrappers",
        "tkinter_filedialog_compat_symbols_present",
        "tkinter_named_variable_uses_explicit_name",
        "tkinter_simpledialog_phase1_exports_present",
        "tkinter_simpledialog_query_helpers",
        "tkinter_set_get_roundtrip",
        "tkinter_tk_available",
        "tkinter_variable_names_deterministic",
        "tkinter_variable_roundtrip",
        "tkinter_variable_set_uses_tk_call",
        "tkinter_widget_wrapper_paths_deterministic",
        "tkinter_widget_wrappers_emit_commands",
        "ttk_all_includes_new_surfaces",
        "ttk_alias_exports_present",
        "ttk_constructor_command_routing",
        "ttk_labeledscale_wrapper_sanity",
        "ttk_notebook_methods_forwarded",
        "ttk_notebook_tab_conflict_error",
        "ttk_new_surfaces_exported",
        "ttk_optionmenu_destroy_releases_variable",
        "ttk_optionmenu_set_menu_updates_values",
        "ttk_panedwindow_methods_forwarded",
        "ttk_scale_configure_emits_range_changed_event",
        "ttk_setup_master_default_root",
        "ttk_style_configure_conflict_error",
        "ttk_style_map_conflict_error",
        "ttk_style_methods_forwarded",
        "ttk_tclobjs_to_py_converts_tuple_like_values",
        "ttk_tkinter_symbol_exported",
        "ttk_tree_column_conflict_error",
        "ttk_treeview_return_fidelity",
        "ttk_treeview_tag_bind_callback_fidelity",
        "ttk_treeview_methods_forwarded",
        "ttk_widget_common_methods_forwarded",
        "ttk_widget_paths_initialized",
        "trace_roundtrip",
        "willdispatch_returns_none",
    }
    missing = sorted(expected - checks.keys())
    assert not missing, f"missing expected headless checks: {missing}"

    failed = sorted(name for name in expected if not checks[name])
    assert not failed, f"headless stub checks failed: {failed}"


def test_ttk_phase0_wrapper_parity_surface_and_methods() -> None:
    lines = _run_probe(_TTK_PHASE0_PARITY_PROBE)
    checks: dict[str, bool] = {}
    for line in lines:
        if not line.startswith("CHECK|"):
            continue
        _, key, raw = line.split("|", 2)
        checks[key] = raw == "True"

    expected = {
        "all",
        "exports",
        "labeledscale",
        "optionmenu_destroy",
        "panedwindow_forget",
        "scale_configure",
        "setup_master",
        "tclobjs_to_py",
        "tkinter_symbol",
    }
    missing = sorted(expected - checks.keys())
    assert not missing, f"missing expected ttk parity checks: {missing}"

    failed = sorted(name for name in expected if not checks[name])
    assert not failed, f"ttk parity checks failed: {failed}"
