"""Phase-0 intrinsic-backed `tkinter.commondialog` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TK_COMMONDIALOG_SHOW = _require_intrinsic("molt_tk_commondialog_show", globals())


def _prepare_intrinsic_options(options):
    if not options:
        return {}
    prepared = dict(options)
    for key in ("parent", "-parent"):
        if key not in prepared:
            continue
        parent = prepared[key]
        if parent is not None:
            prepared[key] = str(parent)
    return prepared


def _resolve_master(master, *, role="dialog master"):
    if master is None:
        return _tkinter._get_default_root()
    if not isinstance(master, _tkinter.Misc):
        raise TypeError(f"{role} must be a tkinter widget or root")
    return master


def _app_handle(master):
    app = master._tk_app
    return getattr(app, "_handle", app)


class Dialog:
    """Minimal common dialog base that forwards to Tk commands."""

    command = None

    def __init__(self, master=None, **options):
        if master is None:
            parent = options.get("parent")
            if isinstance(parent, _tkinter.Misc):
                master = parent
        self.master = master
        self.options = dict(options)

    def _fixoptions(self):
        return None

    def _fixresult(self, widget, result):
        del widget
        return result

    def show(self, **options):
        if options:
            self.options.update(options)
        if not self.command:
            raise RuntimeError("dialog command is not configured")
        master = _resolve_master(self.master, role="dialog master")
        self._fixoptions()
        self._test_callback(master)
        result = _MOLT_TK_COMMONDIALOG_SHOW(
            _app_handle(master),
            str(master),
            self.command,
            _prepare_intrinsic_options(self.options),
        )
        return self._fixresult(master, result)

    def _test_callback(self, master):
        del master
        return None


__all__ = ["Dialog"]
