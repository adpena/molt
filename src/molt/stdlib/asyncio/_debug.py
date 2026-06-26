"""Private debug utilities shared by asyncio shim modules."""

from __future__ import annotations

import os
import sys
from typing import Any


def _env_flag(name: str) -> bool:
    return os.getenv(name) == "1"


def _debug_gather_enabled() -> bool:
    return _env_flag("MOLT_DEBUG_GATHER")


def _debug_wait_for_enabled() -> bool:
    return _env_flag("MOLT_DEBUG_WAIT_FOR")


def _debug_tasks_enabled() -> bool:
    return _env_flag("MOLT_DEBUG_TASKS")


def _debug_asyncio_promise_enabled() -> bool:
    return _env_flag("MOLT_DEBUG_ASYNCIO_PROMISE")


def _debug_asyncio_exc_enabled() -> bool:
    return _env_flag("MOLT_DEBUG_ASYNCIO_EXC")


def _debug_asyncio_condition_enabled() -> bool:
    return _env_flag("MOLT_DEBUG_ASYNCIO_CONDITION")


def _debug_asyncio_handles_enabled() -> bool:
    return _env_flag("MOLT_DEBUG_ASYNCIO_HANDLES")


def _debug_asyncio_shutdown_enabled() -> bool:
    return _env_flag("MOLT_DEBUG_ASYNCIO_SHUTDOWN")


def _debug_write(message: str) -> None:
    err = getattr(sys, "stderr", None)
    if err is None or not hasattr(err, "write"):
        err = getattr(sys, "__stderr__", None)
    if err is not None and hasattr(err, "write"):
        err.write(f"{message}\n")
        flush_fn = getattr(err, "flush", None)
        if callable(flush_fn):
            flush_fn()
        return None
    out = getattr(sys, "stdout", None)
    if out is not None and hasattr(out, "write"):
        out.write(f"{message}\n")
        flush_fn = getattr(out, "flush", None)
        if callable(flush_fn):
            flush_fn()
        return None
    print(message)


def _debug_exc_state(tag: str) -> None:
    if not _debug_asyncio_exc_enabled():
        return None
    asyncio_mod = sys.modules.get("asyncio")
    pending_fn = getattr(asyncio_mod, "_molt_exception_pending", None)
    last_fn = getattr(asyncio_mod, "_molt_exception_last", None)
    pending = pending_fn() if callable(pending_fn) else 0
    last_obj = last_fn() if pending and callable(last_fn) else None
    last_type = (
        getattr(type(last_obj), "__name__", "None") if last_obj is not None else "None"
    )
    _debug_write(
        "asyncio_exc tag={tag} pending={pending} last={last}".format(
            tag=tag, pending=int(bool(pending)), last=last_type
        )
    )
    return None


def _debug_task_summary(task: Any) -> str:
    task_type = type(task).__name__
    task_name_getter = getattr(task, "get_name", None)
    if callable(task_name_getter):
        try:
            task_name = task_name_getter()
        except BaseException as err:
            task_name = f"<name:{type(err).__name__}>"
    else:
        task_name = getattr(task, "_name", None)
    done_getter = getattr(task, "done", None)
    if callable(done_getter):
        try:
            done_state = bool(done_getter())
        except BaseException as err:
            done_state = f"error:{type(err).__name__}"
    else:
        done_state = "<no-done>"
    return "type={task_type} name={task_name!r} done={done_state!r}".format(
        task_type=task_type,
        task_name=task_name,
        done_state=done_state,
    )
