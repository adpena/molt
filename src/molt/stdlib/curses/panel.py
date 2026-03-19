"""Public API surface shim for ``curses.panel``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")


class error(Exception):
    pass


class panel:
    pass


new_panel = len
top_panel = len
bottom_panel = len
update_panels = len
version = "2.0"
