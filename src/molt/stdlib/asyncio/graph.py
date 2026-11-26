"""Version-gated `asyncio.graph` import behavior."""

import sys


def _raise_missing():
    raise ModuleNotFoundError("No module named 'asyncio.graph'")


if getattr(sys, "version_info", (0, 0))[1] < 14:
    _raise_missing()


from .tasks import (
    FrameCallGraphEntry as FrameCallGraphEntry,
    FutureCallGraph as FutureCallGraph,
    capture_call_graph as capture_call_graph,
    format_call_graph as format_call_graph,
    print_call_graph as print_call_graph,
)
