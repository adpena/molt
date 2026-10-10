"""Public API surface shim for ``curses.panel``."""

from __future__ import annotations


class error(Exception):
    pass


class panel:
    pass


new_panel = len
top_panel = len
bottom_panel = len
update_panels = len
version = "2.0"
