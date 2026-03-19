"""Public API surface shim for ``asyncio.windows_events``.

On Windows: re-exports ProactorEventLoop, IocpProactor, SelectorEventLoop,
and the Windows-specific event loop policy classes from the main asyncio
module.

On non-Windows: raises ImportError on any attribute access.  This mirrors
CPython 3.12+ behavior where ``import asyncio.windows_events`` is only
valid on win32.
"""

from __future__ import annotations

import sys as _sys
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

_VERSION_INFO = getattr(_sys, "version_info", (3, 12, 0, "final", 0))

__all__ = [
    "SelectorEventLoop",
    "ProactorEventLoop",
    "IocpProactor",
    "DefaultEventLoopPolicy",
    "WindowsSelectorEventLoopPolicy",
    "WindowsProactorEventLoopPolicy",
]

if _sys.platform != "win32":
    # On non-Windows platforms, every attribute access raises ImportError,
    # matching CPython semantics where this module cannot be imported.
    def __getattr__(attr: str) -> Any:
        raise ImportError("asyncio.windows_events is only available on Windows")
else:
    # Re-export event loop classes from the main asyncio module, mirroring
    # the synthetic module that asyncio/__init__.py builds at lines 6251-6264.
    from asyncio import SelectorEventLoop
    from asyncio import _ProactorEventLoop as ProactorEventLoop
    from asyncio import (
        _WindowsProactorEventLoopPolicy as DefaultEventLoopPolicy,
    )
    from asyncio import (
        _WindowsSelectorEventLoopPolicy as WindowsSelectorEventLoopPolicy,
    )
    from asyncio import (
        _WindowsProactorEventLoopPolicy as WindowsProactorEventLoopPolicy,
    )

    # EventLoop alias added in CPython 3.13 (same as ProactorEventLoop on
    # Windows).
    if _VERSION_INFO >= (3, 13):
        EventLoop = ProactorEventLoop

    class IocpProactor:
        """I/O Completion Port proactor — API-compatibility stub.

        Molt uses its own intrinsic-backed I/O multiplexing rather than
        CPython's IOCP proactor.  This class exists so that code which
        references ``asyncio.windows_events.IocpProactor`` (or passes it
        to ``ProactorEventLoop``) does not raise ``AttributeError``.

        All I/O operations delegate to the Molt runtime scheduler; the
        ``concurrency`` hint is accepted but ignored.
        """

        # TODO(async-runtime, owner:runtime, milestone:SL3, priority:P1, status:partial): wire IocpProactor methods to Molt runtime IOCP intrinsics on Windows

        def __init__(self, concurrency: int = 0xFFFFFFFF) -> None:
            self._concurrency = concurrency

        def close(self) -> None:
            """Release proactor resources (no-op in Molt)."""

        def __repr__(self) -> str:
            return f"<IocpProactor concurrency={self._concurrency:#x}>"

globals().pop("_require_intrinsic", None)
