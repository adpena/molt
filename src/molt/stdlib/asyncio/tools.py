"""asyncio.tools — Task introspection tools (CPython 3.14+).

Provides capture_call_graph, format_call_graph, and print_call_graph for
inspecting the call graph of running asyncio tasks.

On Python < 3.14 this module has no public API.
"""

import sys as _sys

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio.tools re-exports graph introspection functions from asyncio; full parity pending deeper runtime integration.

_VERSION_INFO = getattr(_sys, "version_info", (3, 12, 0, "final", 0))

if _VERSION_INFO >= (3, 14):
    from asyncio import capture_call_graph, format_call_graph, print_call_graph

    __all__ = ["capture_call_graph", "format_call_graph", "print_call_graph"]
else:
    __all__: list[str] = []

    def __getattr__(attr: str):
        raise AttributeError(
            "module 'asyncio.tools' has no attribute %r (requires Python 3.14+)" % attr
        )
