"""_asyncio C-extension surface shim for Molt.

Provides the same public API as CPython's ``_asyncio`` C module:
running-loop accessors, ``Task`` and ``Future`` proxy classes,
task context helpers (``_enter_task``, ``_leave_task``), and task
registration (``_register_task``, ``_unregister_task``).

CPython's ``_asyncio`` C extension exposes accelerated ``Task`` and
``Future`` classes that override the pure-Python versions at import
time.  Molt's implementations in ``asyncio/__init__.py`` are already
intrinsic-backed; the proxies here re-export them so that code
importing from ``_asyncio`` directly sees the correct types.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# ---------------------------------------------------------------------------
# Running-loop intrinsics (existing)
# ---------------------------------------------------------------------------

_MOLT_ASYNCIO_RUNNING_LOOP_GET = _require_intrinsic(
    "molt_asyncio_running_loop_get", globals()
)
_MOLT_ASYNCIO_RUNNING_LOOP_SET = _require_intrinsic(
    "molt_asyncio_running_loop_set", globals()
)
_MOLT_ASYNCIO_EVENT_LOOP_GET = _require_intrinsic(
    "molt_asyncio_event_loop_get", globals()
)
_MOLT_ASYNCIO_EVENT_LOOP_POLICY_GET = _require_intrinsic(
    "molt_asyncio_event_loop_policy_get", globals()
)

# ---------------------------------------------------------------------------
# Task-context intrinsics (new — _asyncio C-extension surface)
# ---------------------------------------------------------------------------

_MOLT_ASYNCIO_TASK_REGISTRY_CURRENT = _require_intrinsic(
    "molt_asyncio_task_registry_current", globals()
)
_MOLT_ASYNCIO_TASK_REGISTRY_CURRENT_FOR_LOOP = _require_intrinsic(
    "molt_asyncio_task_registry_current_for_loop", globals()
)
_MOLT_ASYNCIO_ENTER_TASK = _require_intrinsic("molt_asyncio_enter_task", globals())
_MOLT_ASYNCIO_LEAVE_TASK = _require_intrinsic("molt_asyncio_leave_task", globals())
_MOLT_ASYNCIO_REGISTER_TASK = _require_intrinsic(
    "molt_asyncio_register_task", globals()
)
_MOLT_ASYNCIO_UNREGISTER_TASK = _require_intrinsic(
    "molt_asyncio_unregister_task", globals()
)

# ---------------------------------------------------------------------------
# Public API — running-loop
# ---------------------------------------------------------------------------


def _get_running_loop():
    return _MOLT_ASYNCIO_RUNNING_LOOP_GET()


def _set_running_loop(loop):
    _MOLT_ASYNCIO_RUNNING_LOOP_SET(loop)
    return None


def get_running_loop():
    loop = _get_running_loop()
    if loop is None:
        raise RuntimeError("no running event loop")
    return loop


def get_event_loop():
    loop = _MOLT_ASYNCIO_EVENT_LOOP_GET()
    if loop is not None:
        return loop

    policy = _MOLT_ASYNCIO_EVENT_LOOP_POLICY_GET()
    if policy is None:
        raise RuntimeError(
            "_asyncio event loop policy is unset; initialize policy via "
            "asyncio.set_event_loop_policy(...) before calling "
            "_asyncio.get_event_loop()"
        )
    return policy.get_event_loop()


# ---------------------------------------------------------------------------
# Public API — current task
# ---------------------------------------------------------------------------


def current_task(loop=None):
    """Return the currently running task for *loop*, or ``None``.

    When *loop* is ``None``, attempts to get the running loop.  If no loop
    is running, returns ``None`` without raising — this matches CPython's
    ``_asyncio.current_task()`` C implementation which clears the error
    and returns ``None`` (unlike ``asyncio.current_task()`` which raises
    via ``get_running_loop()``).
    """
    if loop is None:
        loop = _get_running_loop()
        if loop is None:
            return None
        return _MOLT_ASYNCIO_TASK_REGISTRY_CURRENT()
    return _MOLT_ASYNCIO_TASK_REGISTRY_CURRENT_FOR_LOOP(loop)


# ---------------------------------------------------------------------------
# Internal helpers — task enter/leave (used by asyncio.tasks)
# ---------------------------------------------------------------------------


def _enter_task(loop, task):
    """Mark *task* as the current task for *loop*.

    Raises ``RuntimeError`` if another task is already current for
    this loop.  Mirrors CPython ``_asyncio._enter_task``.
    """
    _MOLT_ASYNCIO_ENTER_TASK(loop, task)


def _leave_task(loop, task):
    """Clear *task* as the current task for *loop*.

    Raises ``RuntimeError`` if *task* is not the current task for
    this loop.  Mirrors CPython ``_asyncio._leave_task``.
    """
    _MOLT_ASYNCIO_LEAVE_TASK(loop, task)


# ---------------------------------------------------------------------------
# Internal helpers — task registration (used by asyncio.tasks)
# ---------------------------------------------------------------------------


def _register_task(task):
    """Add *task* to the global set of all tasks.

    Mirrors CPython ``_asyncio._register_task``.
    """
    _MOLT_ASYNCIO_REGISTER_TASK(task)


def _unregister_task(task):
    """Remove *task* from the global set of all tasks.

    Mirrors CPython ``_asyncio._unregister_task``.
    """
    _MOLT_ASYNCIO_UNREGISTER_TASK(task)


# ---------------------------------------------------------------------------
# Task and Future proxy classes
#
# CPython's _asyncio C extension exposes accelerated Task and Future types.
# Molt's canonical implementations live in asyncio/__init__.py; we re-export
# them here so that ``from _asyncio import Task, Future`` works correctly.
# ---------------------------------------------------------------------------

# Lazy import to avoid circular dependency — asyncio/__init__.py imports
# _intrinsics at module level, and _asyncio.py is loaded before asyncio.
_Task = None
_Future = None


def _ensure_asyncio_types():
    global _Task, _Future
    if _Task is None:
        import asyncio

        _Task = asyncio.Task
        _Future = asyncio.Future


class Future:
    """Proxy for ``asyncio.Future``.

    On first instantiation the real asyncio.Future is resolved via lazy
    import and all subsequent operations are forwarded.  This class also
    serves as the type identity for ``isinstance`` checks performed by
    code that imports from ``_asyncio`` directly.
    """

    def __new__(cls, *args, **kwargs):
        _ensure_asyncio_types()
        return _Future(*args, **kwargs)

    def __init_subclass__(cls, **kwargs):
        _ensure_asyncio_types()
        super().__init_subclass__(**kwargs)

    @classmethod
    def __class_getitem__(cls, item):
        _ensure_asyncio_types()
        return _Future.__class_getitem__(item)


class Task(Future):
    """Proxy for ``asyncio.Task``.

    Mirrors CPython's ``_asyncio.Task`` which is the C-accelerated
    implementation of ``asyncio.Task``.
    """

    def __new__(cls, coro, *, loop=None, name=None, context=None, eager_start=None):
        _ensure_asyncio_types()
        return _Task(coro, loop=loop, name=name, context=context)

    @classmethod
    def __class_getitem__(cls, item):
        _ensure_asyncio_types()
        return _Task.__class_getitem__(item)


# ---------------------------------------------------------------------------
# Module exports
# ---------------------------------------------------------------------------

__all__ = [
    "Future",
    "Task",
    "_get_running_loop",
    "_set_running_loop",
    "get_running_loop",
    "get_event_loop",
    "current_task",
    "_enter_task",
    "_leave_task",
    "_register_task",
    "_unregister_task",
]
