"""CPython-only shims for Molt intrinsics used in tests.

These helpers provide a minimal, blocking implementation of channels and task
spawning to keep the Python test suite executable. They are not used by the
compiled runtime or production builds.
"""

from __future__ import annotations

import atexit
import asyncio
import builtins
import queue
import threading
from typing import Any

_loop: asyncio.AbstractEventLoop | None = None
_thread: threading.Thread | None = None


def _run_loop(loop: asyncio.AbstractEventLoop) -> None:
    asyncio.set_event_loop(loop)
    loop.run_forever()


def _ensure_loop() -> asyncio.AbstractEventLoop:
    global _loop, _thread
    if _loop is None:
        _loop = asyncio.new_event_loop()
        _thread = threading.Thread(
            target=_run_loop,
            args=(_loop,),
            name="molt-shim-loop",
            daemon=True,
        )
        _thread.start()
        atexit.register(_shutdown)
    return _loop


def _shutdown() -> None:
    if _loop is None:
        return
    _loop.call_soon_threadsafe(_loop.stop)
    if _thread is not None:
        _thread.join(timeout=1)


def molt_spawn(task: Any) -> None:
    loop = _ensure_loop()
    if asyncio.iscoroutine(task):
        asyncio.run_coroutine_threadsafe(task, loop)
        return
    if callable(task):
        asyncio.run_coroutine_threadsafe(task(), loop)
        return
    raise TypeError("molt_spawn expects a coroutine or callable")


def molt_chan_new() -> queue.Queue[Any]:
    return queue.Queue()


def molt_chan_send(chan: queue.Queue[Any], val: Any) -> int:
    chan.put(val)
    return 0


def molt_chan_recv(chan: queue.Queue[Any]) -> Any:
    return chan.get()


def install() -> None:
    setattr(builtins, "molt_spawn", molt_spawn)
    setattr(builtins, "molt_chan_new", molt_chan_new)
    setattr(builtins, "molt_chan_send", molt_chan_send)
    setattr(builtins, "molt_chan_recv", molt_chan_recv)
