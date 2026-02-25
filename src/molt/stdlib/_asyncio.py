"""_asyncio C-extension surface shim for Molt.

Provides the same public API as CPython's ``_asyncio`` C module:
running-loop accessors, task context helpers (``_enter_task``,
``_leave_task``), and task registration (``_register_task``,
``_unregister_task``).  ``Task`` and ``Future`` are not re-exported
here because Molt's intrinsic-backed implementations already live in
``asyncio/__init__.py``; CPython replaces its pure-Python versions
at import time, but in Molt they are already native-backed from the
start.
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
    """Return the currently running task for *loop*.

    When *loop* is omitted, this matches CPython by requiring an active running
    loop and raising ``RuntimeError`` when called outside one.
    """
    if loop is None:
        loop = _get_running_loop()
        if loop is None:
            raise RuntimeError("no running event loop")
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
# Module exports
# ---------------------------------------------------------------------------

__all__ = [
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
