from __future__ import annotations

import ast
from pathlib import Path

import pytest


# Compiled shutdown acceptance lives in the strict asyncio_task_basic fixture
# selected by runtime.public-consumers.native.
# The native drop call is the boundary under test; no runtime is loaded by these
# controls. Compile the actual finalizer and its defaults, then retire its module
# namespace exactly as module shutdown does. The table describes constructor
# handle ownership, independently of the finalizer implementation.
_HANDLE_FINALIZERS = [
    ("asyncio/events.py", "_EventLoop", "_loop_handle", "_molt_event_loop_drop"),
    (
        "asyncio/events.py",
        "_ReadPipeTransport",
        "_pipe_handle",
        "_molt_pipe_transport_drop",
    ),
    (
        "asyncio/events.py",
        "_WritePipeTransport",
        "_pipe_handle",
        "_molt_pipe_transport_drop",
    ),
    ("asyncio/futures.py", "Future", "_fut_handle", "_molt_asyncio_future_drop"),
    ("asyncio/tasks.py", "CancellationToken", "_token", "_molt_cancel_token_drop"),
    ("asyncio/locks.py", "Event", "_evt_handle", "_molt_asyncio_event_drop"),
    ("asyncio/locks.py", "Lock", "_lock_handle", "_molt_asyncio_lock_drop"),
    ("asyncio/locks.py", "Semaphore", "_sem_handle", "_molt_asyncio_semaphore_drop"),
    ("asyncio/queues.py", "Queue", "_q_handle", "_molt_asyncio_queue_drop"),
    ("asyncio/streams.py", "StreamReader", "_reader", "_molt_socket_reader_drop"),
    (
        "asyncio/streams.py",
        "ProcessStreamReader",
        "_reader",
        "_molt_stream_reader_drop",
    ),
    ("asyncio/subprocess.py", "Process", "_handle", "_molt_process_drop"),
    ("concurrent/futures/__init__.py", "Future", "_handle", "_MOLT_FUTURE_DROP"),
    (
        "concurrent/futures/__init__.py",
        "ThreadPoolExecutor",
        "_handle",
        "_MOLT_THREADPOOL_DROP",
    ),
]


def _handle_finalizer_class(relative_path, class_name, drop_name, drop):
    path = Path(__file__).resolve().parents[1] / "src/molt/stdlib" / relative_path
    tree = ast.parse(path.read_text(encoding="utf-8"))
    owner = next(
        node
        for node in tree.body
        if isinstance(node, ast.ClassDef) and node.name == class_name
    )
    finalizer = next(
        node
        for node in owner.body
        if isinstance(node, ast.FunctionDef) and node.name == "__del__"
    )
    unit = ast.parse("from __future__ import annotations\nclass FinalizerOwner: pass\n")
    wrapper = unit.body[1]
    assert isinstance(wrapper, ast.ClassDef)
    wrapper.body = [finalizer]
    namespace = {drop_name: drop, "_PROCESS_WAIT_FUTURES": {}}
    exec(compile(ast.fix_missing_locations(unit), str(path), "exec"), namespace)
    return namespace["FinalizerOwner"], namespace


@pytest.mark.parametrize("relative_path,class_name,field,drop_name", _HANDLE_FINALIZERS)
def test_handle_finalizer_survives_retired_globals_exactly_once(
    relative_path, class_name, field, drop_name
):
    calls = []
    instance = None

    def drop(handle):
        calls.append(handle)
        # A native owner's retirement can release Python payloads and reenter.
        # The handle must have been detached before that callback.
        instance.__del__()

    owner, namespace = _handle_finalizer_class(
        relative_path, class_name, drop_name, drop
    )
    instance = owner()
    setattr(instance, field, 73)
    if class_name == "CancellationToken":
        instance._owned = True
    if class_name == "ThreadPoolExecutor":
        instance._shutdown = False
    wait_futures = namespace["_PROCESS_WAIT_FUTURES"]
    wait_futures[id(instance)] = object()
    namespace.clear()
    instance.__del__()
    instance.__del__()
    assert calls == [73]
    if class_name == "Process":
        assert id(instance) not in wait_futures
    owner().__del__()  # constructor failed before publishing a handle
    assert calls == [73]


@pytest.mark.parametrize(
    "relative_path,class_name,field,drop_name",
    [
        row
        for row in _HANDLE_FINALIZERS
        if row[1] in {"CancellationToken", "ThreadPoolExecutor"}
    ],
)
def test_handle_finalizer_does_not_drop_borrowed_or_shutdown_owner(
    relative_path, class_name, field, drop_name
):
    calls = []
    owner, namespace = _handle_finalizer_class(
        relative_path, class_name, drop_name, calls.append
    )
    instance = owner()
    setattr(instance, field, 73)
    if class_name == "CancellationToken":
        instance._owned = False
    else:
        instance._shutdown = True
    namespace.clear()
    instance.__del__()
    assert calls == []


def test_handle_finalizer_process_detaches_before_wait_future_retirement():
    calls = []
    row = next(row for row in _HANDLE_FINALIZERS if row[1] == "Process")
    owner, namespace = _handle_finalizer_class(row[0], row[1], row[3], calls.append)
    instance = owner()
    instance._handle = 73

    class WaitFuture:
        def __del__(self):
            instance.__del__()

    wait_futures = namespace["_PROCESS_WAIT_FUTURES"]
    wait_futures[id(instance)] = WaitFuture()
    namespace.clear()
    instance.__del__()
    assert calls == [73]
    assert wait_futures == {}


def test_handle_finalizer_preserves_drop_failure_and_prevents_duplicate_retirement():
    error = RuntimeError("native drop witness")
    calls = []

    def drop(handle):
        calls.append(handle)
        raise error

    row = next(row for row in _HANDLE_FINALIZERS if row[1] == "Queue")
    owner, namespace = _handle_finalizer_class(row[0], row[1], row[3], drop)
    instance = owner()
    instance._q_handle = 73
    namespace.clear()
    with pytest.raises(RuntimeError) as raised:
        instance.__del__()
    assert raised.value is error
    instance.__del__()
    assert calls == [73]


@pytest.mark.parametrize("class_name", ["_ReadPipeTransport", "_WritePipeTransport"])
def test_handle_finalizer_preserves_missing_pipe_intrinsic_diagnostic(class_name):
    owner, namespace = _handle_finalizer_class(
        "asyncio/events.py", class_name, "_molt_pipe_transport_drop", None
    )
    namespace.clear()
    instance = owner()
    instance.__del__()
    instance._pipe_handle = 73
    with pytest.raises(
        RuntimeError, match="^asyncio intrinsic not available: pipe_transport_drop$"
    ):
        instance.__del__()
    instance.__del__()
