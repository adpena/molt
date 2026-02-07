"""Minimal _asyncio shim for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement
# full _asyncio C-accelerated surface on top of runtime intrinsics.

from _intrinsics import require_intrinsic as _require_intrinsic

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
        import asyncio as _asyncio

        policy = _asyncio.get_event_loop_policy()
    return policy.get_event_loop()


__all__ = [
    "_get_running_loop",
    "_set_running_loop",
    "get_running_loop",
    "get_event_loop",
]
