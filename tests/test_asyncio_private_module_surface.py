from __future__ import annotations

import sys
import ast
import builtins
import types
from pathlib import Path

import pytest

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"


@pytest.mark.parametrize("version", [(3, 12), (3, 13), (3, 14), (3, 15)])
def test_call_graph_producers_and_reexports_follow_target_version(
    monkeypatch: pytest.MonkeyPatch, version: tuple[int, int]
) -> None:
    import asyncio

    source = ast.parse((STDLIB_ROOT / "asyncio/tasks.py").read_text(encoding="utf-8"))
    selector = next(
        node
        for node in source.body
        if isinstance(node, ast.Assign)
        and any(
            isinstance(target, ast.Name) and target.id == "_EXPOSE_GRAPH"
            for target in node.targets
        )
    )
    producer = next(
        node
        for node in source.body
        if isinstance(node, ast.If)
        and any(
            isinstance(child, ast.ClassDef) and child.name == "FutureCallGraph"
            for child in node.body
        )
    )
    imports: list[str] = []

    def imported(name: str, *args: object, **kwargs: object) -> object:
        imports.append(name)
        return builtins.__import__(name, *args, **kwargs)

    def require(name: str) -> object:
        assert name == "molt_sys_version_info"
        return lambda: (*version, 0, "final", 0)

    tasks = types.ModuleType("_molt_call_graph_surface")
    monkeypatch.setitem(sys.modules, tasks.__name__, tasks)
    tasks.__dict__.update(
        __builtins__=vars(builtins) | {"__import__": imported},
        _require_intrinsic=require,
        _types=types,
        _sys=sys,
        Future=asyncio.Future,
    )
    # Execute the actual source producer and target selector, rather than a
    # duplicate implementation of the versioned classes or their decorators.
    unit = ast.Module(body=[source.body[1], selector, producer], type_ignores=[])
    exec(compile(unit, "asyncio/tasks.py", "exec"), tasks.__dict__)
    present = version >= (3, 14)
    assert ("dataclasses" in imports) is present
    names = (
        "FrameCallGraphEntry",
        "FutureCallGraph",
        "capture_call_graph",
        "format_call_graph",
        "print_call_graph",
    )
    assert all(hasattr(tasks, name) is present for name in names)
    if present:
        entry = tasks.FrameCallGraphEntry(sys._getframe())
        graph = tasks.FutureCallGraph(None, (entry,), ())
        assert graph.call_stack == (entry,)
        assert graph.awaited_by == ()
        assert not hasattr(graph, "__dict__")
        with pytest.raises(AttributeError):
            graph.future = object()
        loop = asyncio.new_event_loop()
        try:
            future = asyncio.Future(loop=loop)
            future._asyncio_awaited_by = ()
            captured = tasks._build_graph_for_future(future)
            assert captured.future is future
            assert captured.call_stack == ()
            assert captured.awaited_by == ()
        finally:
            loop.close()

    package = types.ModuleType("_molt_call_graph_package")
    package.__path__ = []
    monkeypatch.setitem(sys.modules, package.__name__, package)
    monkeypatch.setitem(sys.modules, package.__name__ + ".tasks", tasks)
    exports = types.ModuleType(package.__name__ + ".exports")
    exports.__package__ = package.__name__
    exports._EXPOSE_GRAPH = present
    root = ast.parse((STDLIB_ROOT / "asyncio/__init__.py").read_text(encoding="utf-8"))
    reexport = next(
        node
        for node in root.body
        if isinstance(node, ast.If)
        and any(
            isinstance(child, ast.ImportFrom) and child.module == "tasks"
            for child in node.body
        )
    )
    exec(
        compile(
            ast.Module(body=[reexport], type_ignores=[]), "asyncio/__init__.py", "exec"
        ),
        exports.__dict__,
    )
    assert all(hasattr(exports, name) is present for name in names)
    if present:
        assert all(getattr(exports, name) is getattr(tasks, name) for name in names)


_PROBE = f"""
import importlib.util
import sys
import types
from tests.stdlib_intrinsic_registry import install_registry


class _AsyncFuture:
    def __init__(self, *args, **kwargs):
        self.args = args
        self.kwargs = kwargs


class _AsyncTask:
    def __init__(self, coro, *, loop=None, name=None, context=None):
        self.coro = coro
        self.loop = loop
        self.name = name
        self.context = context


_fake_asyncio = types.ModuleType("asyncio")
_fake_asyncio.Future = _AsyncFuture
_fake_asyncio.Task = _AsyncTask
# _asyncio.get_event_loop delegates to the one asyncio.events implementation.
_fake_events = types.ModuleType("asyncio.events")
_fake_events.get_event_loop = lambda: ("events", _state["event_loop"])
_fake_asyncio.events = _fake_events
sys.modules["asyncio"] = _fake_asyncio
sys.modules["asyncio.events"] = _fake_events

_state = {{"running_loop": None, "event_loop": "event-loop", "current": "current-task"}}

install_registry({{
    "molt_asyncio_running_loop_get": lambda: _state["running_loop"],
    "molt_asyncio_running_loop_set": lambda loop: _state.__setitem__("running_loop", loop),
    "molt_asyncio_task_registry_current": lambda: _state["current"],
    "molt_asyncio_task_registry_current_for_loop": lambda loop: ("for-loop", loop),
    "molt_asyncio_enter_task": lambda loop, task: _state.__setitem__("current", task),
    "molt_asyncio_leave_task": lambda loop, task: _state.__setitem__("current", None),
    "molt_asyncio_register_task": lambda task: None,
    "molt_asyncio_unregister_task": lambda task: None,
}})


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_private = _load_module("_molt_private_asyncio", {str(STDLIB_ROOT / "_asyncio.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

_private._set_running_loop("loop-a")
_private._enter_task("loop-a", "task-a")
checks = {{
    "behavior": (
        _private._get_running_loop() == "loop-a"
        and _private.get_running_loop() == "loop-a"
        and _private.get_event_loop() == ("events", "event-loop")
        and _private.current_task() == "task-a"
        and _private.current_task("loop-a") == ("for-loop", "loop-a")
        and isinstance(_private.Future(), _AsyncFuture)
        and isinstance(_private.Task("coro", loop="loop-a"), _AsyncTask)
    ),
    "private_handles_hidden": (
        "_MOLT_ASYNCIO_RUNNING_LOOP_GET" not in _private.__dict__
        and "_MOLT_ASYNCIO_RUNNING_LOOP_SET" not in _private.__dict__
        and "_MOLT_ASYNCIO_TASK_REGISTRY_CURRENT" not in _private.__dict__
        and "_MOLT_ASYNCIO_TASK_REGISTRY_CURRENT_FOR_LOOP" not in _private.__dict__
        and "_MOLT_ASYNCIO_ENTER_TASK" not in _private.__dict__
        and "_MOLT_ASYNCIO_LEAVE_TASK" not in _private.__dict__
        and "_MOLT_ASYNCIO_REGISTER_TASK" not in _private.__dict__
        and "_MOLT_ASYNCIO_UNREGISTER_TASK" not in _private.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = run_surface_test_process(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__asyncio_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_asyncio_enter_task" not in names
    assert "Future" in names
    assert "Task" in names
    assert "current_task" in names
    assert "get_event_loop" in names
    assert "get_running_loop" in names
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}


_STREAMS_PROBE = f"""
import asyncio as reference_asyncio
import asyncio.streams as reference_streams
import errno
import importlib.util
import socket
import sys
import types


def unreached(*args, **kwargs):
    raise AssertionError("unexpected stream intrinsic")


fake_asyncio = types.ModuleType("asyncio")
fake_asyncio.FlowControlMixin = reference_streams.FlowControlMixin
fake_asyncio.IncompleteReadError = reference_asyncio.IncompleteReadError
fake_asyncio.Protocol = reference_asyncio.Protocol
fake_asyncio._SOCKET = socket
fake_asyncio._errno = errno
for name in (
    "_io_wait", "_molt_socket_reader_at_eof", "_molt_socket_reader_drop",
    "_molt_socket_reader_new", "_molt_stream_close", "_molt_stream_reader_at_eof",
    "_molt_stream_reader_drop", "_molt_stream_reader_new",
    "_require_asyncio_intrinsic", "_require_io_wait_new",
    "_require_unix_socket_support", "_socket_module", "_socket_wait_key",
    "_tls_client_from_fd", "_tls_server_from_fd", "_tls_server_payload",
    "_molt_asyncio_server_accept_loop_new", "_molt_asyncio_socket_reader_read_new",
    "_molt_asyncio_socket_reader_readline_new", "_molt_asyncio_stream_buffer_consume",
    "_molt_asyncio_stream_buffer_snapshot", "_molt_asyncio_stream_reader_read_new",
    "_molt_asyncio_stream_reader_readline_new", "_molt_asyncio_stream_send_all_new",
    "sleep",
):
    setattr(fake_asyncio, name, unreached)
fake_asyncio._require_ssl_transport_support = lambda *args, **kwargs: True
current_error = None


def tls_connect(host, port, server_hostname):
    assert (host, port, server_hostname) == ("identity.invalid", 443, "identity.invalid")
    assert current_error is not None
    raise current_error


fake_asyncio._tls_client_connect = tls_connect
# No retired numeric resolver mapper exists in this import boundary.
sys.modules["asyncio"] = fake_asyncio
spec = importlib.util.spec_from_file_location(
    "_molt_streams_identity", {str(STDLIB_ROOT / "asyncio" / "streams.py")!r}
)
assert spec is not None and spec.loader is not None
streams = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = streams
spec.loader.exec_module(streams)
assert "_map_socket_name_resolution_error" not in streams.__dict__
for error in (
    socket.gaierror(9, "resolver collision"),
    OSError(9, "ordinary EBADF collision"),
    BlockingIOError(errno.EAGAIN, "ordinary would block"),
):
    current_error = error
    operation = streams.open_connection("identity.invalid", 443, ssl=True)
    try:
        operation.send(None)
    except OSError as observed:
        assert observed is error
        assert type(observed) is type(error)
        assert observed.errno == error.errno
    else:
        raise AssertionError("TLS failure was not propagated")
    finally:
        operation.close()
print("streams-import-and-exception-identity: ok")
"""


def test_streams_import_preserves_canonical_tls_exception_identity() -> None:
    proc = run_surface_test_process(
        [sys.executable, "-c", _STREAMS_PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    assert proc.stdout.strip() == "streams-import-and-exception-identity: ok"
