"""Purpose: focused `_tkinter`/`tkinter` Phase-0 + wrapper surface checks."""

# MOLT_ENV: MOLT_TRUSTED=0 MOLT_CAPABILITIES=

from __future__ import annotations

import importlib
import importlib.util
from types import ModuleType

SENTINEL_ATTR = "__molt_tk_phase0_missing_attr__"
MODULES = ("_tkinter", "tkinter")
SUBMODULES = (
    "tkinter.__main__",
    "tkinter.colorchooser",
    "tkinter.commondialog",
    "tkinter.constants",
    "tkinter.dialog",
    "tkinter.dnd",
    "tkinter.filedialog",
    "tkinter.font",
    "tkinter.messagebox",
    "tkinter.scrolledtext",
    "tkinter.simpledialog",
    "tkinter.tix",
    "tkinter.ttk",
)
ALLOWED_IMPORT_ERROR_TYPES = frozenset(
    {
        "ImportError",
        "ModuleNotFoundError",
        "NotImplementedError",
        "OSError",
        "PermissionError",
        "RuntimeError",
        "TclError",
    }
)


def _report(module_name: str, label: str, value: object) -> None:
    print(f"{module_name}:{label}:{value}")


def _import_signature(
    module_name: str,
) -> tuple[str, ModuleType | None, BaseException | None]:
    try:
        spec = importlib.util.find_spec(module_name)
    except BaseException as exc:  # noqa: BLE001
        return "spec_error", None, exc
    if spec is None:
        return "spec_missing", None, None
    try:
        module = importlib.import_module(module_name)
    except BaseException as exc:  # noqa: BLE001
        return "import_error", None, exc
    return "imported", module, None


def _import_contract_ok(status: str, exc: BaseException | None) -> bool:
    if status in {"imported", "spec_missing"}:
        return True
    if exc is None:
        return False
    return type(exc).__name__ in ALLOWED_IMPORT_ERROR_TYPES


def _probe_missing_attr(module_name: str, module: ModuleType) -> bool:
    try:
        getattr(module, SENTINEL_ATTR)
    except BaseException as exc:  # noqa: BLE001
        return type(exc).__name__ == "AttributeError" and SENTINEL_ATTR in str(exc)
    return False


def _tkinter_phase0_core_ok(module: ModuleType | None) -> bool:
    if module is None:
        # Host CPython may not ship Tk support in `_tkinter`.
        return True
    if not _is_molt_phase0__tkinter(module):
        # CPython's builtin `_tkinter` shape differs from Molt's phase-0 contract.
        return True
    required_callables = [
        "create",
        "getboolean",
        "getdouble",
        "getint",
        "splitlist",
    ]
    required_callables.extend(
        [
            "call",
            "after",
            "dooneevent",
            "mainloop",
            "quit",
            "setvar",
            "getvar",
            "unsetvar",
            "tk_available",
            "has_gui_capability",
            "has_process_spawn_capability",
        ]
    )
    for name in required_callables:
        if not callable(getattr(module, name, None)):
            return False
    if not isinstance(getattr(module, "TkappType", None), type):
        return False
    if not isinstance(getattr(module, "Tcl_Obj", None), type):
        return False
    for const_name in (
        "TK_VERSION",
        "TCL_VERSION",
        "READABLE",
        "WRITABLE",
        "EXCEPTION",
        "ALL_EVENTS",
    ):
        if not hasattr(module, const_name):
            return False
    return True


def _expect_permission_error(callback: object, needle: str) -> bool:
    if not callable(callback):
        return False
    try:
        callback()
    except BaseException as exc:  # noqa: BLE001
        return isinstance(exc, PermissionError) and needle in str(exc)
    return False


def _probe_tkinter_capability_guards(module: ModuleType | None) -> bool:
    if module is None:
        return True
    gui_gate = getattr(module, "_require_gui_window_capability", None)
    gui_probe = getattr(module, "_HAS_GUI_CAPABILITY", None)
    capability_probe = getattr(module, "_MOLT_CAPABILITIES_HAS", None)
    if callable(gui_gate) and callable(gui_probe) and callable(capability_probe):
        previous = gui_probe
        previous_capability = capability_probe
        try:
            module._HAS_GUI_CAPABILITY = lambda: False
            module._MOLT_CAPABILITIES_HAS = lambda _name=None: False
            if not _expect_permission_error(gui_gate, "gui.window"):
                return False
        finally:
            module._HAS_GUI_CAPABILITY = previous
            module._MOLT_CAPABILITIES_HAS = previous_capability

    process_gate = getattr(module, "_require_process_spawn_capability", None)
    process_probe = getattr(module, "_HAS_PROCESS_SPAWN_CAPABILITY", None)
    capability_probe = getattr(module, "_MOLT_CAPABILITIES_HAS", None)
    if (
        callable(process_gate)
        and callable(process_probe)
        and callable(capability_probe)
    ):
        previous = process_probe
        previous_capability = capability_probe
        try:
            module._HAS_PROCESS_SPAWN_CAPABILITY = lambda: False
            module._MOLT_CAPABILITIES_HAS = lambda _name=None: False
            if not _expect_permission_error(process_gate, "process.spawn"):
                return False
        finally:
            module._HAS_PROCESS_SPAWN_CAPABILITY = previous
            module._MOLT_CAPABILITIES_HAS = previous_capability
    return True


def _probe_ttk_capability_guard(module: ModuleType | None) -> bool:
    if module is None:
        return True
    gate = getattr(module, "_require_gui_capability", None)
    probe = getattr(module, "_MOLT_CAPABILITIES_HAS", None)
    if callable(gate) and callable(probe):
        previous = probe
        try:
            module._MOLT_CAPABILITIES_HAS = lambda _name: False
            if not _expect_permission_error(gate, "gui.window"):
                return False
        finally:
            module._MOLT_CAPABILITIES_HAS = previous
    return True


def _is_molt_phase0_tkinter(module: ModuleType | None) -> bool:
    return module is not None and callable(
        getattr(module, "_require_gui_window_capability", None)
    )


def _is_molt_phase0__tkinter(module: ModuleType | None) -> bool:
    return (
        module is not None
        and isinstance(getattr(module, "TkappType", None), type)
        and hasattr(module, "_MOLT_TK_APP_NEW")
        and hasattr(module, "_MOLT_TK_CALL")
    )


def _probe_runtime_error_contract(
    tk_module: ModuleType | None,
    tkinter_module: ModuleType | None,
) -> bool:
    if not _is_molt_phase0__tkinter(tk_module) or not _is_molt_phase0_tkinter(
        tkinter_module
    ):
        return True

    def _raise_runtime(_options=None):
        raise RuntimeError("tk runtime unavailable (probe)")

    old_tk_available = tk_module._MOLT_TK_AVAILABLE
    old_app_new = tk_module._MOLT_TK_APP_NEW
    old_gui_gate = tkinter_module._require_gui_window_capability
    old_process_gate = tkinter_module._require_process_spawn_capability
    try:
        tk_module._MOLT_TK_AVAILABLE = lambda: False
        tk_module._MOLT_TK_APP_NEW = _raise_runtime
        tkinter_module._require_gui_window_capability = lambda: None
        tkinter_module._require_process_spawn_capability = lambda: None
        if tk_module.tk_available() is not False:
            return False
        if tkinter_module.tk_available() is not False:
            return False
        try:
            tkinter_module.Tk(useTk=False)
        except BaseException as exc:  # noqa: BLE001
            return isinstance(exc, RuntimeError)
        return False
    finally:
        tk_module._MOLT_TK_AVAILABLE = old_tk_available
        tk_module._MOLT_TK_APP_NEW = old_app_new
        tkinter_module._require_gui_window_capability = old_gui_gate
        tkinter_module._require_process_spawn_capability = old_process_gate


def _probe_headless_stubbed_semantics(
    tk_module: ModuleType | None,
    tkinter_module: ModuleType | None,
) -> bool:
    if not _is_molt_phase0__tkinter(tk_module) or not _is_molt_phase0_tkinter(
        tkinter_module
    ):
        return True

    app_runtime_attrs = {
        "_MOLT_TK_AVAILABLE": tk_module._MOLT_TK_AVAILABLE,
        "_MOLT_TK_APP_NEW": tk_module._MOLT_TK_APP_NEW,
        "_MOLT_TK_QUIT": tk_module._MOLT_TK_QUIT,
        "_MOLT_TK_MAINLOOP": tk_module._MOLT_TK_MAINLOOP,
        "_MOLT_TK_DO_ONE_EVENT": tk_module._MOLT_TK_DO_ONE_EVENT,
        "_MOLT_TK_AFTER": tk_module._MOLT_TK_AFTER,
        "_MOLT_TK_CALL": tk_module._MOLT_TK_CALL,
        "_MOLT_TK_BIND_COMMAND": tk_module._MOLT_TK_BIND_COMMAND,
        "_MOLT_TK_DESTROY_WIDGET": tk_module._MOLT_TK_DESTROY_WIDGET,
        "_MOLT_TK_LAST_ERROR": tk_module._MOLT_TK_LAST_ERROR,
    }
    old_gui_gate = tkinter_module._require_gui_window_capability
    old_process_gate = tkinter_module._require_process_spawn_capability
    dialog_module = None
    simpledialog_module = None
    old_dialog_show = None
    old_simpledialog_query = None
    root = None
    try:

        def _app_new(_opts=None):
            create_options = dict(_opts or {})
            return {
                "vars": {},
                "commands": {},
                "calls": [],
                "last_error": None,
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
                name = str(argv[1])
                if name not in app["commands"]:
                    raise RuntimeError(f'invalid command name "{name}"')
                app["commands"].pop(name, None)
                return ""
            if op == "after":
                return "after#stub"
            if op == "tkwait" and len(argv) >= 3 and str(argv[1]) == "window":
                for command_name, callback in tuple(app["commands"].items()):
                    if "simpledialog_ok" in str(command_name):
                        callback()
                        break
                return ""
            return tuple(argv)

        def _tk_bind_command(app, name, callback):
            app["commands"][str(name)] = callback

        def _tk_after(_app, delay_ms, callback):
            callback()
            return f"after#{int(delay_ms)}"

        tk_module._MOLT_TK_AVAILABLE = lambda: True
        tk_module._MOLT_TK_APP_NEW = _app_new
        tk_module._MOLT_TK_QUIT = lambda _app=None: None
        tk_module._MOLT_TK_MAINLOOP = lambda _app=None: None
        tk_module._MOLT_TK_DO_ONE_EVENT = lambda _app=None, _flags=None: False
        tk_module._MOLT_TK_AFTER = _tk_after
        tk_module._MOLT_TK_CALL = _tk_call
        tk_module._MOLT_TK_BIND_COMMAND = _tk_bind_command
        tk_module._MOLT_TK_DESTROY_WIDGET = lambda _app=None, _widget=None: None
        tk_module._MOLT_TK_LAST_ERROR = (
            lambda app=None: None if app is None else app.get("last_error")
        )
        tkinter_module._require_gui_window_capability = lambda: None
        tkinter_module._require_process_spawn_capability = lambda: None

        app = tk_module.create(useTk=False)
        if not tk_module.tk_available():
            return False
        if not tkinter_module.tk_available():
            return False
        if app._handle["create_options"].get("useTk") is not False:
            return False
        if tk_module.setvar(app, "phase0", "value") != "value":
            return False
        if tk_module.getvar(app, "phase0") != "value":
            return False

        tk_module.createcommand(app, "phase0_cmd", lambda: None)
        tk_module.deletecommand(app, "phase0_cmd")
        if ("rename", "phase0_cmd", "") not in app._handle["calls"]:
            return False

        app._handle["last_error"] = "headless runtime error"
        if tk_module.last_error(app) != "headless runtime error":
            return False
        after_events: list[str] = []
        after_token = tk_module.after(app, 7, lambda: after_events.append("fired"))
        if after_token != "after#7" or after_events != ["fired"]:
            return False

        root = tkinter_module.Tk(useTk=False)
        if root._tk_app._handle["create_options"].get("useTk") is not False:
            return False
        root.setvar("phase0_root", "root-value")
        if root.getvar("phase0_root") != "root-value":
            return False
        if root.call("set", "phase0_call", "call-value") != "call-value":
            return False
        if root.call("set", "phase0_call") != "call-value":
            return False
        root.createcommand("phase0_root_cmd", lambda: None)
        root.deletecommand("phase0_root_cmd")
        if ("rename", "phase0_root_cmd", "") not in root._tk_app._handle["calls"]:
            return False
        tk_after_events: list[str] = []
        tk_after_token = root.after(9, lambda: tk_after_events.append("root-fired"))
        if tk_after_token != "after#9" or tk_after_events != ["root-fired"]:
            return False
        dialog_module = importlib.import_module("tkinter.dialog")
        simpledialog_module = importlib.import_module("tkinter.simpledialog")
        old_dialog_show = getattr(dialog_module, "_MOLT_TK_DIALOG_SHOW", None)
        old_simpledialog_query = getattr(
            simpledialog_module, "_MOLT_TK_SIMPLEDIALOG_QUERY", None
        )

        def _dialog_show_stub(
            _app,
            _master_path,
            _title,
            _text,
            _bitmap,
            default_index,
            strings,
        ):
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

        def _simpledialog_query_stub(
            _app,
            _parent_path,
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
            return None

        dialog_module._MOLT_TK_DIALOG_SHOW = _dialog_show_stub
        simpledialog_module._MOLT_TK_SIMPLEDIALOG_QUERY = _simpledialog_query_stub
        if (
            simpledialog_module.askstring(
                "Probe",
                "Enter value",
                parent=root,
                initialvalue="alpha",
            )
            != "alpha"
        ):
            return False
        if (
            simpledialog_module.askinteger(
                "Probe",
                "Enter integer",
                parent=root,
                initialvalue="7",
                minvalue=1,
                maxvalue=9,
            )
            != 7
        ):
            return False
        if (
            simpledialog_module.askfloat(
                "Probe",
                "Enter float",
                parent=root,
                initialvalue="2.5",
                minvalue=1.0,
                maxvalue=3.0,
            )
            != 2.5
        ):
            return False
        if (
            simpledialog_module.askinteger(
                "Probe",
                "Enter integer",
                parent=root,
                initialvalue="not-an-int",
            )
            is not None
        ):
            return False
        root.destroy()
        root = None
        return True
    except BaseException:  # noqa: BLE001
        return False
    finally:
        if root is not None:
            try:
                root.destroy()
            except BaseException:  # noqa: BLE001
                pass
        for attr_name, attr_value in app_runtime_attrs.items():
            setattr(tk_module, attr_name, attr_value)
        tkinter_module._require_gui_window_capability = old_gui_gate
        tkinter_module._require_process_spawn_capability = old_process_gate
        if dialog_module is not None and old_dialog_show is not None:
            dialog_module._MOLT_TK_DIALOG_SHOW = old_dialog_show
        if simpledialog_module is not None and old_simpledialog_query is not None:
            simpledialog_module._MOLT_TK_SIMPLEDIALOG_QUERY = old_simpledialog_query


def _probe_ttk_treeview_headless_semantics(
    tk_module: ModuleType | None,
    tkinter_module: ModuleType | None,
    ttk_module: ModuleType | None,
) -> bool:
    if (
        not _is_molt_phase0__tkinter(tk_module)
        or not _is_molt_phase0_tkinter(tkinter_module)
        or ttk_module is None
    ):
        return True

    app_runtime_attrs = {
        "_MOLT_TK_AVAILABLE": tk_module._MOLT_TK_AVAILABLE,
        "_MOLT_TK_APP_NEW": tk_module._MOLT_TK_APP_NEW,
        "_MOLT_TK_QUIT": tk_module._MOLT_TK_QUIT,
        "_MOLT_TK_MAINLOOP": tk_module._MOLT_TK_MAINLOOP,
        "_MOLT_TK_DO_ONE_EVENT": tk_module._MOLT_TK_DO_ONE_EVENT,
        "_MOLT_TK_AFTER": tk_module._MOLT_TK_AFTER,
        "_MOLT_TK_CALL": tk_module._MOLT_TK_CALL,
        "_MOLT_TK_BIND_COMMAND": tk_module._MOLT_TK_BIND_COMMAND,
        "_MOLT_TK_DESTROY_WIDGET": tk_module._MOLT_TK_DESTROY_WIDGET,
        "_MOLT_TK_LAST_ERROR": tk_module._MOLT_TK_LAST_ERROR,
    }
    old_gui_gate = tkinter_module._require_gui_window_capability
    old_process_gate = tkinter_module._require_process_spawn_capability
    old_ttk_capability_probe = getattr(ttk_module, "_MOLT_CAPABILITIES_HAS", None)
    root = None
    try:

        def _app_new(_opts=None):
            create_options = dict(_opts or {})
            return {
                "vars": {},
                "commands": {},
                "calls": [],
                "last_error": None,
                "create_options": create_options,
                "tree_tag_bindings": {},
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
                name = str(argv[1])
                if name not in app["commands"]:
                    raise RuntimeError(f'invalid command name "{name}"')
                app["commands"].pop(name, None)
                return ""
            if op == "after":
                return "after#stub"
            if len(argv) >= 2 and str(argv[1]) == "bbox":
                return "1 2 3 4"
            if len(argv) >= 2 and str(argv[1]) == "children":
                if len(argv) == 3 and argv[2] == "":
                    return ("row1", "row2")
                if len(argv) == 3 and argv[2] == "row0":
                    return ("row1",)
                return ""
            if len(argv) >= 2 and str(argv[1]) == "exists":
                return "1"
            if len(argv) >= 2 and str(argv[1]) == "selection":
                if len(argv) == 2:
                    return ("row1",)
                return ""
            if len(argv) >= 2 and str(argv[1]) == "index":
                return "7"
            if len(argv) >= 2 and str(argv[1]) == "set":
                if len(argv) == 3:
                    return ("#0", "row-text", "value", "row-value")
                if len(argv) == 4:
                    return "row-value"
                return ""
            if len(argv) >= 3 and str(argv[1]) == "tag" and str(argv[2]) == "has":
                if len(argv) == 4:
                    return ("row1", "row2")
                return "1"
            if len(argv) >= 3 and str(argv[1]) == "tag" and str(argv[2]) == "bind":
                tagname = str(argv[3]) if len(argv) >= 4 else ""
                sequence = str(argv[4]) if len(argv) >= 5 else ""
                key = (op, tagname, sequence)
                if len(argv) == 5:
                    return app["tree_tag_bindings"].get(key, "")
                if len(argv) >= 6:
                    app["tree_tag_bindings"][key] = str(argv[5])
                    return ""
            return tuple(argv)

        def _tk_bind_command(app, name, callback):
            app["commands"][str(name)] = callback

        def _tk_after(_app, delay_ms, callback):
            callback()
            return f"after#{int(delay_ms)}"

        tk_module._MOLT_TK_AVAILABLE = lambda: True
        tk_module._MOLT_TK_APP_NEW = _app_new
        tk_module._MOLT_TK_QUIT = lambda _app=None: None
        tk_module._MOLT_TK_MAINLOOP = lambda _app=None: None
        tk_module._MOLT_TK_DO_ONE_EVENT = lambda _app=None, _flags=None: False
        tk_module._MOLT_TK_AFTER = _tk_after
        tk_module._MOLT_TK_CALL = _tk_call
        tk_module._MOLT_TK_BIND_COMMAND = _tk_bind_command
        tk_module._MOLT_TK_DESTROY_WIDGET = lambda _app=None, _widget=None: None
        tk_module._MOLT_TK_LAST_ERROR = (
            lambda app=None: None if app is None else app.get("last_error")
        )
        tkinter_module._require_gui_window_capability = lambda: None
        tkinter_module._require_process_spawn_capability = lambda: None
        if callable(old_ttk_capability_probe):
            ttk_module._MOLT_CAPABILITIES_HAS = lambda _name: True

        root = tkinter_module.Tk(useTk=False)
        tree = ttk_module.Treeview(root)
        bbox = tree.bbox("row0")
        children_root = tree.get_children()
        children_row0 = tree.get_children("row0")
        tree.set_children("row0", "row1", "row2")
        exists_row0 = tree.exists("row0")
        selection = tree.selection()
        tree_index = tree.index("row0")
        set_all = tree.set("row1")
        set_single = tree.set("row1", "value")
        tree.set("row1", "value", "cell-value")
        tag_has_all = tree.tag_has("tag1")
        tag_has_row1 = tree.tag_has("tag1", "row1")

        events = []
        payload = []

        def _on_tag(event):
            events.append(getattr(event, "widget", None) is tree)
            payload.append(
                (
                    getattr(event, "x", None),
                    getattr(event, "y", None),
                    getattr(event, "delta", None),
                )
            )

        funcid = tree.tag_bind("tag1", "<<TreeviewOpen>>", _on_tag)
        query_before = tree.tag_bind("tag1", "<<TreeviewOpen>>")
        cmd = root._tk_app._handle["commands"].get(funcid)
        if cmd is None:
            return False
        cmd()
        cmd(object())
        cmd(
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
            tree._w,
            "VirtualEvent",
            "115",
            "116",
            "120",
        )
        tree.tag_unbind("tag1", "<<TreeviewOpen>>", funcid)
        query_after = tree.tag_bind("tag1", "<<TreeviewOpen>>")

        calls = root._tk_app._handle["calls"]
        registered_commands = root._tk_app._handle["commands"]
        root.destroy()
        root = None
        return (
            isinstance(bbox, tuple)
            and bbox == (1, 2, 3, 4)
            and isinstance(children_root, tuple)
            and isinstance(children_row0, tuple)
            and exists_row0 is True
            and isinstance(selection, tuple)
            and tree_index == 7
            and isinstance(set_all, dict)
            and set_all.get("#0") == "row-text"
            and set_all.get("value") == "row-value"
            and set_single == "row-value"
            and isinstance(tag_has_all, tuple)
            and tag_has_row1 is True
            and isinstance(funcid, str)
            and isinstance(query_before, str)
            and funcid in query_before
            and query_after == ""
            and funcid not in registered_commands
            and events == [True, False, True]
            and payload[-1] == (15, 16, 120)
            and any(call[:3] == (tree._w, "children", "") for call in calls)
            and any(
                call[:4] == (tree._w, "children", "row0", ("row1", "row2"))
                for call in calls
            )
        )
    except BaseException:  # noqa: BLE001
        return False
    finally:
        if root is not None:
            try:
                root.destroy()
            except BaseException:  # noqa: BLE001
                pass
        for attr_name, attr_value in app_runtime_attrs.items():
            setattr(tk_module, attr_name, attr_value)
        tkinter_module._require_gui_window_capability = old_gui_gate
        tkinter_module._require_process_spawn_capability = old_process_gate
        if callable(old_ttk_capability_probe):
            ttk_module._MOLT_CAPABILITIES_HAS = old_ttk_capability_probe


def main() -> None:
    imported: dict[str, ModuleType | None] = {}
    for module_name in MODULES:
        status, module, error = _import_signature(module_name)
        imported[module_name] = module
        _report(module_name, "import_contract", _import_contract_ok(status, error))
        if module is not None:
            _report(
                module_name,
                "missing_attr_contract",
                _probe_missing_attr(module_name, module),
            )
        else:
            _report(module_name, "missing_attr_contract", True)
        if module_name == "_tkinter":
            _report(module_name, "phase0_core_api", _tkinter_phase0_core_ok(module))
        if module_name == "tkinter":
            _report(
                module_name,
                "capability_gate_contract",
                _probe_tkinter_capability_guards(module),
            )

    _report(
        "tkinter",
        "runtime_error_contract",
        _probe_runtime_error_contract(
            imported.get("_tkinter"), imported.get("tkinter")
        ),
    )
    _report(
        "tkinter",
        "headless_stub_semantics",
        _probe_headless_stubbed_semantics(
            imported.get("_tkinter"), imported.get("tkinter")
        ),
    )

    for module_name in SUBMODULES:
        status, module, error = _import_signature(module_name)
        _report(module_name, "import_contract", _import_contract_ok(status, error))
        if module is not None:
            _report(
                module_name,
                "missing_attr_contract",
                _probe_missing_attr(module_name, module),
            )
        else:
            _report(module_name, "missing_attr_contract", True)
        if module_name == "tkinter.ttk":
            _report(
                module_name,
                "capability_gate_contract",
                _probe_ttk_capability_guard(module),
            )
            _report(
                module_name,
                "treeview_headless_semantics",
                _probe_ttk_treeview_headless_semantics(
                    imported.get("_tkinter"), imported.get("tkinter"), module
                ),
            )


if __name__ == "__main__":
    main()
