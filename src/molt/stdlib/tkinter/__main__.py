"""Intrinsic-backed entrypoint for `tkinter`."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

from ._support import tk_unavailable_message as _tk_unavailable_message


def _lazy_intrinsic(name):
    def _call(*args, **kwargs):
        return _require_intrinsic(name, globals())(*args, **kwargs)

    return _call


_MOLT_CAPABILITIES_HAS = _lazy_intrinsic("molt_capabilities_has")
_MOLT_TK_AVAILABLE = _lazy_intrinsic("molt_tk_available")


def _has_gui_capability():
    return bool(_MOLT_CAPABILITIES_HAS("gui.window")) or bool(
        _MOLT_CAPABILITIES_HAS("gui")
    )


def _require_gui_capability():
    if not _has_gui_capability():
        raise PermissionError("missing gui.window capability")


def _require_tk_runtime(operation):
    if bool(_MOLT_TK_AVAILABLE()):
        return
    raise RuntimeError(_tk_unavailable_message(operation))


def _test():
    _require_gui_capability()
    _require_tk_runtime("tkinter.__main__._test")
    root = _tkinter.Tk()
    try:
        root.mainloop()
    finally:
        root.destroy()


def main():
    _test()


if __name__ == "__main__":
    main()
