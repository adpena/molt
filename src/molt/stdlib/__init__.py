"""Capability-gated stdlib stubs for Molt."""

from __future__ import annotations

from molt.stdlib import contextlib as contextlib
from molt.stdlib.io import open as open
from molt.stdlib.io import stream as stream
from molt.stdlib.pathlib import Path

__all__ = ["Path", "contextlib", "open", "stream"]
