from __future__ import annotations

import re


_ANSI_CSI_RE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def strip_terminal_decoration(text: str) -> str:
    """Remove terminal presentation escapes from machine-consumed diagnostics."""

    return _ANSI_CSI_RE.sub("", text)
