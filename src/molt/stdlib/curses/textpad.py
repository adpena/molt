"""Public API surface shim for ``curses.textpad``."""

from __future__ import annotations


from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")


class Textbox:
    def __init__(self, win, insert_mode: bool = False):
        self.win = win
        self.insert_mode = bool(insert_mode)

    def edit(self, validator=None):
        del validator
        return ""

    def gather(self):
        return ""


def rectangle(win, uly, ulx, lry, lrx):
    del win, uly, ulx, lry, lrx
    return None
