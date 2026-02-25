"""Phase-0 intrinsic-backed `tkinter.dnd` wrappers."""

from _intrinsics import require_intrinsic as _require_intrinsic

from ._support import tk_unavailable_message as _tk_unavailable_message

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has", globals())
_MOLT_TK_AVAILABLE = _require_intrinsic("molt_tk_available", globals())


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


class DndHandler:
    """Minimal drag-and-drop state holder."""

    def __init__(self, source, event):
        self.source = source
        self.initial_widget = getattr(event, "widget", None)
        self.target = None

    def on_motion(self, event):
        del event
        return self.target

    def on_release(self, event):
        return self.finish(event, commit=True)

    def cancel(self, event=None):
        return self.finish(event, commit=False)

    def finish(self, event=None, commit=False):
        target = self.target
        if target is None:
            return None
        leave = getattr(target, "dnd_leave", None)
        if callable(leave):
            leave(self.source, event)
        if commit:
            commit_fn = getattr(target, "dnd_commit", None)
            if callable(commit_fn):
                commit_fn(self.source, event)
        end = getattr(target, "dnd_end", None)
        if callable(end):
            end(self.source, event)
        self.target = None
        return target


def dnd_start(source, event):
    if source is None:
        raise TypeError("source must not be None")
    _require_gui_capability()
    _require_tk_runtime("tkinter.dnd.dnd_start")
    return DndHandler(source, event)


__all__ = ["DndHandler", "dnd_start"]
