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
    if module_name.endswith(".__main__"):
        # Entry-point modules like `tkinter.__main__` intentionally execute
        # interactive/demo code on import in CPython. For this differential we
        # only care that the module resolves through importlib, not that its
        # side-effectful entrypoint runs.
        return "spec_only", None, None
    try:
        module = importlib.import_module(module_name)
    except BaseException as exc:  # noqa: BLE001
        return "import_error", None, exc
    return "imported", module, None


def _import_contract_ok(status: str, exc: BaseException | None) -> bool:
    if status in {"imported", "spec_missing", "spec_only"}:
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
        "_MOLT_TK_UNBIND_COMMAND": tk_module._MOLT_TK_UNBIND_COMMAND,
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

        def _tk_unbind_command(app, name):
            command_name = str(name)
            if command_name not in app["commands"]:
                raise RuntimeError(f'invalid command name "{command_name}"')
            app["commands"].pop(command_name, None)
            app["calls"].append(("rename", command_name, ""))
            return None

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
        tk_module._MOLT_TK_UNBIND_COMMAND = _tk_unbind_command
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


def _probe_core_runtime_semantics(
    tk_module: ModuleType | None,
    tkinter_module: ModuleType | None,
) -> bool:
    if not _is_molt_phase0__tkinter(tk_module) or not _is_molt_phase0_tkinter(
        tkinter_module
    ):
        return True

    old_gui_gate = tkinter_module._require_gui_window_capability
    old_process_gate = tkinter_module._require_process_spawn_capability
    root = None
    try:
        tkinter_module._require_gui_window_capability = lambda: None
        tkinter_module._require_process_spawn_capability = lambda: None

        root = tkinter_module.Tk(useTk=False)
        frame = tkinter_module.Frame(root, width=111, height=77)

        frame.pack(side="left")
        pack_info = frame.pack_info()
        pack_slaves = tuple(root.pack_slaves())
        frame.pack_forget()

        frame.grid(row=1, column=2)
        grid_info = frame.grid_info()
        grid_size = root.grid_size()
        frame.grid_remove()

        frame.place(x=5, y=6)
        place_info = frame.place_info()
        place_slaves = tuple(root.place_slaves())
        frame.place_forget()

        root.title("Probe Title")
        wm_title = root.title()
        root.geometry("320x200+3+4")
        wm_geometry = root.geometry()
        root.state("normal")
        wm_state = root.state()
        root.attributes(alpha=0.5)
        wm_alpha = root.attributes("-alpha")
        root.resizable(False, True)
        wm_resizable = root.resizable()
        root.minsize(100, 80)
        wm_minsize = root.minsize()
        root.maxsize(900, 700)
        wm_maxsize = root.maxsize()
        root.overrideredirect(True)
        wm_overrideredirect = root.overrideredirect()
        root.transient(root)
        wm_transient = root.transient()
        root.iconname("probe-icon")
        wm_iconname = root.iconname()
        proto_id = root.protocol("WM_DELETE_WINDOW", lambda: None)
        proto_before_clear = root.protocol("WM_DELETE_WINDOW")
        root.protocol("WM_DELETE_WINDOW", "")
        proto_after_clear = root.protocol("WM_DELETE_WINDOW")

        key_payload_primary = []
        key_payload_secondary = []

        def _event_payload(event):
            return (
                getattr(event, "widget", None) is frame,
                getattr(event, "x", None),
                getattr(event, "y", None),
                getattr(event, "delta", None),
                getattr(event, "keysym", None),
                getattr(event, "char", None),
                getattr(event, "serial", None),
                getattr(event, "type", None),
                getattr(event, "x_root", None),
                getattr(event, "y_root", None),
            )

        def _on_key_primary(event):
            key_payload_primary.append(_event_payload(event))
            return "break"

        def _on_key_secondary(event):
            key_payload_secondary.append(_event_payload(event))
            return "break"

        bind_id_primary = frame.bind("<KeyPress>", _on_key_primary)
        bind_id_secondary = frame.bind("<KeyPress>", _on_key_secondary, add="+")
        bind_before = frame.bind("<KeyPress>")
        root.tk.call(
            bind_id_primary,
            "55",
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
            frame._w,
            "KeyPress",
            "115",
            "116",
            "120",
        )
        root.tk.call(
            bind_id_secondary,
            "56",
            "1",
            "1",
            "10",
            "11",
            "12",
            "13",
            "14",
            "25",
            "26",
            "B",
            "1",
            "L",
            "66",
            frame._w,
            "KeyPress",
            "215",
            "216",
            "80",
        )
        bind_after = frame.bind("<KeyPress>")
        frame.unbind("<KeyPress>", bind_id_primary)
        bind_after_primary_unbind = frame.bind("<KeyPress>")
        bind_registered_after_primary_unbind = set(
            getattr(root, "_registered_commands", ())
        )
        root.tk.call(
            bind_id_secondary,
            "57",
            "1",
            "1",
            "10",
            "11",
            "12",
            "13",
            "14",
            "35",
            "36",
            "C",
            "1",
            "M",
            "67",
            frame._w,
            "KeyPress",
            "315",
            "316",
            "40",
        )
        frame.unbind("<KeyPress>", bind_id_secondary)
        bind_after_unbind = frame.bind("<KeyPress>")
        bind_registered_after_unbind = set(getattr(root, "_registered_commands", ()))

        frame.event_add("<<ProbeVirtual>>", "<KeyPress>")
        virtuals_before = tuple(frame.event_info())
        virtual_probe_before = tuple(frame.event_info("<<ProbeVirtual>>"))
        frame.event_delete("<<ProbeVirtual>>", "<KeyPress>")
        virtual_probe_after = tuple(frame.event_info("<<ProbeVirtual>>"))

        frame.focus_set()
        focus_current = root.focus_get()

        frame.grab_set()
        grab_current = frame.grab_current()
        grab_status = frame.grab_status()
        frame.grab_release()
        grab_after = frame.grab_current()

        root.clipboard_clear()
        root.clipboard_append("clip-value")
        clipboard_text = root.clipboard_get()
        root.selection_clear()
        selection_text = root.selection_get()

        winfo_exists = frame.winfo_exists()
        winfo_manager = frame.winfo_manager()
        winfo_class = frame.winfo_class()
        winfo_name = frame.winfo_name()
        winfo_parent = frame.winfo_parent()
        winfo_toplevel = frame.winfo_toplevel()
        winfo_id = frame.winfo_id()
        winfo_rgb = frame.winfo_rgb("#112233")
        atom_id = root.winfo_atom("MOLT_PROBE_ATOM")
        atom_name = root.winfo_atomname(atom_id)
        containing = root.winfo_containing(0, 0)
        winfo_children = tuple(root.winfo_children())

        def _normalize_trace_info_rows(rows):
            normalized = []
            for row in rows:
                if not isinstance(row, (tuple, list)) or len(row) != 2:
                    return None
                mode_value = row[0]
                if isinstance(mode_value, (tuple, list)):
                    if len(mode_value) != 1:
                        return None
                    mode_value = mode_value[0]
                mode_name = str(mode_value)
                if mode_name == "w":
                    mode_name = "write"
                normalized.append((mode_name, str(row[1])))
            return tuple(normalized)

        trace_events = []
        trace_var = tkinter_module.StringVar(root, value="seed")

        def _make_trace_callback(label):
            def _callback(name, index, mode):
                trace_events.append((label, name, index, mode))

            return _callback

        trace_id_primary = trace_var.trace_add("write", _make_trace_callback("primary"))
        trace_id_secondary = trace_var.trace_add(
            "write", _make_trace_callback("secondary")
        )
        trace_info_before = tuple(trace_var.trace_info())
        trace_var.set("seed-updated")
        trace_var.trace_remove("write", trace_id_primary)
        trace_info_after_primary_remove = tuple(trace_var.trace_info())
        trace_var.set("seed-final")
        trace_var.trace_remove("write", trace_id_secondary)
        trace_info_after = tuple(trace_var.trace_info())
        trace_info_before_rows = _normalize_trace_info_rows(trace_info_before)
        trace_info_after_primary_rows = _normalize_trace_info_rows(
            trace_info_after_primary_remove
        )
        trace_info_after_rows = _normalize_trace_info_rows(trace_info_after)
        trace_event_labels = tuple(label for label, _name, _index, _mode in trace_events)
        trace_event_names = tuple(name for _label, name, _index, _mode in trace_events)
        trace_event_indexes = tuple(index for _label, _name, index, _mode in trace_events)
        trace_event_modes = tuple(
            "write" if str(mode) == "w" else str(mode)
            for _label, _name, _index, mode in trace_events
        )

        wait_var = tkinter_module.StringVar(root, value="pending")
        root.after(5, lambda: wait_var.set("ready"))
        root.wait_variable(wait_var)
        wait_var_value = wait_var.get()

        wait_window_events = []
        wait_window_target = tkinter_module.Frame(root)
        wait_window_target.pack()
        root.after(5, lambda: (wait_window_events.append("destroyed"), wait_window_target.destroy()))
        root.wait_window(wait_window_target)
        wait_window_missing_result = root.wait_window(".__molt_missing_window__")

        wait_visibility_events = []
        wait_visibility_target = tkinter_module.Frame(root)
        root.after(5, lambda: (wait_visibility_events.append("visible"), wait_visibility_target.pack()))
        root.wait_visibility(wait_visibility_target)
        wait_visibility_target.pack_forget()

        wait_visibility_error_type = ""
        wait_visibility_error_text = ""
        try:
            root.wait_visibility(".__molt_missing_window__")
        except BaseException as exc:  # noqa: BLE001
            wait_visibility_error_type = type(exc).__name__
            wait_visibility_error_text = str(exc)

        after_cancel_hits = []
        cancel_token = root.after(50, lambda: after_cancel_hits.append("fired"))
        root.after_cancel(cancel_token)
        for _ in range(200):
            root.dooneevent(tkinter_module.DONT_WAIT)
        after_cancel_ok = after_cancel_hits == []

        root.update()
        root.update_idletasks()

        root.destroy()
        root = None

        return (
            isinstance(pack_info, (tuple, list, dict))
            and str(frame) in tuple(str(x) for x in pack_slaves)
            and isinstance(grid_info, (tuple, list, dict))
            and isinstance(grid_size, tuple)
            and len(grid_size) == 2
            and isinstance(place_info, (tuple, list, dict))
            and str(frame) in tuple(str(x) for x in place_slaves)
            and wm_title == "Probe Title"
            and wm_geometry == "320x200+3+4"
            and wm_state == "normal"
            and str(wm_alpha).startswith("0.5")
            and wm_resizable == (False, True)
            and wm_minsize == (100, 80)
            and wm_maxsize == (900, 700)
            and wm_overrideredirect is True
            and wm_transient == "."
            and wm_iconname == "probe-icon"
            and isinstance(proto_id, str)
            and proto_before_clear == proto_id
            and proto_after_clear == ""
            and isinstance(bind_id_primary, str)
            and isinstance(bind_id_secondary, str)
            and bind_id_primary in str(bind_before)
            and bind_id_secondary in str(bind_before)
            and bind_after == bind_before
            and bind_id_primary not in str(bind_after_primary_unbind)
            and bind_id_secondary in str(bind_after_primary_unbind)
            and bind_after_unbind == ""
            and bind_id_primary not in bind_registered_after_primary_unbind
            and bind_id_secondary in bind_registered_after_primary_unbind
            and bind_id_primary not in bind_registered_after_unbind
            and bind_id_secondary not in bind_registered_after_unbind
            and key_payload_primary == [
                (True, 15, 16, 120, "K", "A", 55, "KeyPress", 115, 116)
            ]
            and key_payload_secondary == [
                (True, 25, 26, 80, "L", "B", 56, "KeyPress", 215, 216),
                (True, 35, 36, 40, "M", "C", 57, "KeyPress", 315, 316),
            ]
            and "<<ProbeVirtual>>" in tuple(str(x) for x in virtuals_before)
            and "<KeyPress>" in tuple(str(x) for x in virtual_probe_before)
            and virtual_probe_after == ()
            and str(focus_current) == str(frame)
            and str(grab_current) == str(frame)
            and grab_status == "local"
            and grab_after == ""
            and clipboard_text == "clip-value"
            and selection_text == ""
            and winfo_exists is True
            and winfo_manager == ""
            and winfo_class == "Frame"
            and isinstance(winfo_name, str)
            and winfo_name
            and winfo_parent == "."
            and winfo_toplevel == "."
            and isinstance(winfo_id, int)
            and isinstance(winfo_rgb, tuple)
            and len(winfo_rgb) == 3
            and atom_name == "MOLT_PROBE_ATOM"
            and isinstance(containing, str)
            and str(frame) in tuple(str(x) for x in winfo_children)
            and trace_event_labels == ("primary", "secondary", "secondary")
            and trace_event_names == (trace_var._name, trace_var._name, trace_var._name)
            and trace_event_indexes == ("", "", "")
            and trace_event_modes == ("write", "write", "write")
            and trace_info_before_rows
            == (("write", trace_id_primary), ("write", trace_id_secondary))
            and trace_info_after_primary_rows == (("write", trace_id_secondary),)
            and trace_info_after_rows == ()
            and wait_var_value == "ready"
            and wait_window_events == ["destroyed"]
            and wait_window_missing_result is None
            and wait_visibility_events == ["visible"]
            and wait_visibility_error_type in ("TclError", "RuntimeError")
            and "bad window path name" in wait_visibility_error_text
            and after_cancel_ok is True
        )
    except BaseException:  # noqa: BLE001
        return False
    finally:
        if root is not None:
            try:
                root.destroy()
            except BaseException:  # noqa: BLE001
                pass
        tkinter_module._require_gui_window_capability = old_gui_gate
        tkinter_module._require_process_spawn_capability = old_process_gate


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
        "_MOLT_TK_UNBIND_COMMAND": tk_module._MOLT_TK_UNBIND_COMMAND,
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
                    script = str(argv[5])
                    if len(argv) >= 7:
                        command_name = str(argv[6])
                        if not script:
                            script = app["tree_tag_bindings"].get(key, "")
                        prefix = f'if {{"[{command_name} '
                        kept = []
                        for line in script.split("\n"):
                            stripped = line.strip()
                            if not stripped:
                                continue
                            if stripped.startswith(prefix):
                                continue
                            kept.append(stripped)
                        script = "\n".join(kept)
                    app["tree_tag_bindings"][key] = script
                    return ""
            return tuple(argv)

        def _tk_bind_command(app, name, callback):
            app["commands"][str(name)] = callback

        def _tk_unbind_command(app, name):
            command_name = str(name)
            if command_name not in app["commands"]:
                raise RuntimeError(f'invalid command name "{command_name}"')
            app["commands"].pop(command_name, None)
            app["calls"].append(("rename", command_name, ""))
            return None

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
        tk_module._MOLT_TK_UNBIND_COMMAND = _tk_unbind_command
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

        tag_primary = []
        tag_secondary = []

        def _tag_event_payload(event):
            return (
                getattr(event, "widget", None) is tree,
                getattr(event, "x", None),
                getattr(event, "y", None),
                getattr(event, "delta", None),
                getattr(event, "keysym", None),
                getattr(event, "char", None),
                getattr(event, "serial", None),
                getattr(event, "type", None),
                getattr(event, "x_root", None),
                getattr(event, "y_root", None),
            )

        def _on_tag_primary(event):
            tag_primary.append(_tag_event_payload(event))

        def _on_tag_secondary(event):
            tag_secondary.append(_tag_event_payload(event))

        funcid_primary = tree.tag_bind("tag1", "<<TreeviewOpen>>", _on_tag_primary)
        query_primary = tree.tag_bind("tag1", "<<TreeviewOpen>>")
        funcid_secondary = tree.tag_bind("tag1", "<<TreeviewOpen>>", _on_tag_secondary)
        query_secondary = tree.tag_bind("tag1", "<<TreeviewOpen>>")
        combined_script = "\n".join(
            [line for line in (str(query_primary).strip(), str(query_secondary).strip()) if line]
        )
        tree.tag_bind("tag1", "<<TreeviewOpen>>", combined_script)
        query_before = tree.tag_bind("tag1", "<<TreeviewOpen>>")

        cmd_primary = root._tk_app._handle["commands"].get(funcid_primary)
        cmd_secondary = root._tk_app._handle["commands"].get(funcid_secondary)
        if cmd_primary is None or cmd_secondary is None:
            return False

        cmd_primary(
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
        cmd_secondary(
            "8",
            "1",
            "1",
            "10",
            "11",
            "12",
            "13",
            "14",
            "25",
            "26",
            "B",
            "1",
            "L",
            "66",
            tree._w,
            "VirtualEvent",
            "215",
            "216",
            "80",
        )
        tree.tag_unbind("tag1", "<<TreeviewOpen>>", funcid_primary)
        query_after_primary_unbind = tree.tag_bind("tag1", "<<TreeviewOpen>>")
        commands_after_primary_unbind = set(root._tk_app._handle["commands"])
        cmd_secondary(
            "9",
            "1",
            "1",
            "10",
            "11",
            "12",
            "13",
            "14",
            "35",
            "36",
            "C",
            "1",
            "M",
            "67",
            tree._w,
            "VirtualEvent",
            "315",
            "316",
            "40",
        )
        tree.tag_unbind("tag1", "<<TreeviewOpen>>", funcid_secondary)
        query_after = tree.tag_bind("tag1", "<<TreeviewOpen>>")
        commands_after_unbind = set(root._tk_app._handle["commands"])

        calls = root._tk_app._handle["calls"]
        registered_commands = set(root._tk_app._handle["commands"])
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
            and isinstance(funcid_primary, str)
            and isinstance(funcid_secondary, str)
            and isinstance(query_before, str)
            and funcid_primary in query_before
            and funcid_secondary in query_before
            and isinstance(query_after_primary_unbind, str)
            and funcid_primary not in query_after_primary_unbind
            and funcid_secondary in query_after_primary_unbind
            and query_after == ""
            and funcid_primary not in commands_after_primary_unbind
            and funcid_secondary in commands_after_primary_unbind
            and funcid_primary not in commands_after_unbind
            and funcid_secondary not in commands_after_unbind
            and funcid_primary not in registered_commands
            and funcid_secondary not in registered_commands
            and tag_primary == [
                (True, 15, 16, 120, "K", "A", 7, "VirtualEvent", 115, 116)
            ]
            and tag_secondary == [
                (True, 25, 26, 80, "L", "B", 8, "VirtualEvent", 215, 216),
                (True, 35, 36, 40, "M", "C", 9, "VirtualEvent", 315, 316),
            ]
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


def _probe_ttk_runtime_semantics(
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

    old_gui_gate = tkinter_module._require_gui_window_capability
    old_process_gate = tkinter_module._require_process_spawn_capability
    old_ttk_capability_probe = getattr(ttk_module, "_MOLT_CAPABILITIES_HAS", None)
    root = None
    try:
        tkinter_module._require_gui_window_capability = lambda: None
        tkinter_module._require_process_spawn_capability = lambda: None
        if callable(old_ttk_capability_probe):
            ttk_module._MOLT_CAPABILITIES_HAS = lambda _name: True

        root = tkinter_module.Tk(useTk=False)

        style = ttk_module.Style(root)
        style.configure("Probe.TButton", padding=4)
        style_padding = style.configure("Probe.TButton", query_opt="padding")
        style.map("Probe.TButton", foreground=[("active", "blue")])
        style_map = style.map("Probe.TButton", query_opt="foreground")
        style_lookup = style.lookup(
            "Probe.TButton", "foreground", ("active",), "fallback"
        )
        style.layout("Probe.TButton", [("Button.border", {"sticky": "nswe"})])
        style_layout = style.layout("Probe.TButton")
        style.element_create(
            "Probe.indicator",
            "from",
            "default",
            "Button.button",
            "-padding",
            "2",
        )
        style_elements = tuple(root.tk.splitlist(style.element_names()))
        style_element_options = tuple(
            root.tk.splitlist(style.element_options("Probe.indicator"))
        )
        style.theme_create("probe_theme", parent="default", settings={"x": 1})
        style.theme_settings("probe_theme", {"y": 2})
        style_theme_names = tuple(root.tk.splitlist(style.theme_names()))
        style.theme_use("probe_theme")
        style_theme_current = style.theme_use()

        notebook = ttk_module.Notebook(root)
        tab_a = ttk_module.Frame(notebook)
        tab_b = ttk_module.Frame(notebook)
        notebook.add(tab_a, text="A")
        notebook.insert("end", tab_b, text="B")
        notebook_tabs_before = tuple(root.tk.splitlist(notebook.tabs()))
        notebook_end_index = notebook.index("end")
        notebook.select(tab_a)
        notebook_selected = notebook.select()
        notebook.tab(tab_a, text="AA")
        notebook_tab_text = notebook.tab(tab_a, option="text")
        notebook.enable_traversal()
        notebook.hide(tab_b)
        notebook_tabs_hidden = tuple(root.tk.splitlist(notebook.tabs()))
        notebook.forget(tab_a)
        notebook_tabs_after = tuple(root.tk.splitlist(notebook.tabs()))

        paned = ttk_module.PanedWindow(root)
        pane = ttk_module.Frame(paned)
        paned.insert("end", pane, weight=1)
        pane_weight_before = paned.pane(pane, option="weight")
        paned.pane(pane, weight=2)
        pane_weight_after = paned.pane(pane, option="weight")
        _ = paned.sashpos(0)
        paned.sashpos(0, 9)
        sash_position = paned.sashpos(0)

        combo = ttk_module.Combobox(root)
        combo.current(2)
        combo_index = combo.current()
        combo.set("combo-value")
        combo_value = combo.tk.call(combo._w, "set")

        progress = ttk_module.Progressbar(root)
        progress.start(17)
        progress.step(3)
        progress.stop()

        scale = ttk_module.Scale(root)
        scale_value = scale.get()

        spin = ttk_module.Spinbox(root)
        spin.set("spin-value")
        spin_value = spin.tk.call(spin._w, "set")

        entry = ttk_module.Entry(root)
        entry_valid = entry.validate()
        entry_bbox = entry.bbox(0)

        root.destroy()
        root = None

        return (
            style_padding == 4
            and isinstance(style_map, (tuple, list))
            and style_lookup is not None
            and style_layout is not None
            and "Probe.indicator" in tuple(str(x) for x in style_elements)
            and "-padding" in tuple(str(x) for x in style_element_options)
            and "probe_theme" in tuple(str(x) for x in style_theme_names)
            and style_theme_current == "probe_theme"
            and len(notebook_tabs_before) == 2
            and notebook_end_index == 2
            and str(notebook_selected) == str(tab_a)
            and notebook_tab_text == "AA"
            and str(tab_b) not in tuple(str(x) for x in notebook_tabs_hidden)
            and str(tab_a) not in tuple(str(x) for x in notebook_tabs_after)
            and pane_weight_before == 1
            and pane_weight_after == 2
            and sash_position == 9
            and combo_index == 2
            and combo_value == "combo-value"
            and scale_value is not None
            and spin_value == "spin-value"
            and bool(entry_valid) is True
            and isinstance(entry_bbox, tuple)
        )
    except BaseException:  # noqa: BLE001
        return False
    finally:
        if root is not None:
            try:
                root.destroy()
            except BaseException:  # noqa: BLE001
                pass
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
    _report(
        "tkinter",
        "runtime_core_semantics",
        _probe_core_runtime_semantics(
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
            _report(
                module_name,
                "runtime_semantics",
                _probe_ttk_runtime_semantics(
                    imported.get("_tkinter"), imported.get("tkinter"), module
                ),
            )


if __name__ == "__main__":
    main()
