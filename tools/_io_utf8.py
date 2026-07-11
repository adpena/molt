#!/usr/bin/env python3
"""One primitive against the Windows cp1252 stdio-relay crash: ``force_utf8_stdio()``.

THE RUNTIME BACKSTOP
--------------------
On Windows ``sys.stdout``/``sys.stderr`` decode/encode with the platform codec
(cp1252), not UTF-8. A tool that ``print()``-relays captured subprocess output
containing a non-cp1252 character (an em-dash, a box-drawing glyph, U+FFFD) then
aborts with ``UnicodeEncodeError`` — throwing away whatever long-running work it
was reporting on (M43 — a ~20-minute witness build died exactly this way).

``tools/encoding_gate.py`` is the ENFORCEMENT (it fails closed on unsafe file /
subprocess call sites). This module is the RUNTIME BACKSTOP: the highest-traffic
entry tools that relay subprocess output call ``force_utf8_stdio()`` once at
import so a stray byte degrades to ``\\xNN`` (backslashreplace) instead of a
crash. Do NOT sprinkle it everywhere — the gate is the guarantee; this is belt.

It is idempotent, guarded for non-``TextIOWrapper`` streams (redirected pipes,
``io.StringIO`` under pytest capture), and never raises.
"""

from __future__ import annotations

import sys
from typing import IO


def force_utf8_stdio(*, errors: str = "backslashreplace") -> None:
    """Reconfigure ``sys.stdout``/``sys.stderr`` to UTF-8; idempotent, never raises.

    ``errors="backslashreplace"`` is deliberate: an un-encodable char becomes a
    visible ``\\xNN`` escape rather than aborting the process — a relayed build
    log must never be able to kill the build. Streams that do not support
    ``reconfigure`` (already-wrapped pipes, ``StringIO`` capture) are skipped.
    """
    for stream in (sys.stdout, sys.stderr):
        _reconfigure(stream, errors)


def _reconfigure(stream: IO[str] | None, errors: str) -> None:
    reconfigure = getattr(stream, "reconfigure", None)
    if reconfigure is None:
        return  # not a TextIOWrapper (redirected pipe, StringIO, None) — nothing to do
    try:
        reconfigure(encoding="utf-8", errors=errors)
    except (AttributeError, ValueError, OSError):
        # ValueError: stream detached; OSError: closed fd. Best-effort backstop —
        # a hardening helper must never itself become the failure.
        pass
