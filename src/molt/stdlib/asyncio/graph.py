"""Version-gated `asyncio.graph` import behavior."""

import sys


def _raise_missing():
    raise ModuleNotFoundError("No module named 'asyncio.graph'")


if getattr(sys, "version_info", (0, 0))[1] < 14:
    _raise_missing()
