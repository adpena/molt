"""Shared tkinter capability/runtime gating helpers."""

from _intrinsics import require_intrinsic as _require_intrinsic


def _lazy_intrinsic(name):
    def _call(*args, **kwargs):
        return _require_intrinsic(name, globals())(*args, **kwargs)

    return _call


_MOLT_CAPABILITIES_HAS = _lazy_intrinsic("molt_capabilities_has")
_MOLT_TK_AVAILABLE = _lazy_intrinsic("molt_tk_available")
_MOLT_TK_LAST_ERROR = _lazy_intrinsic("molt_tk_last_error")


def has_gui_capability():
    return bool(_MOLT_CAPABILITIES_HAS("gui.window")) or bool(
        _MOLT_CAPABILITIES_HAS("gui")
    )


def has_process_spawn_capability():
    return bool(_MOLT_CAPABILITIES_HAS("process.spawn")) or bool(
        _MOLT_CAPABILITIES_HAS("process")
    )


def require_gui_capability():
    if not has_gui_capability():
        raise PermissionError("missing gui.window capability")


def require_process_spawn_capability():
    if not has_process_spawn_capability():
        raise PermissionError("missing process.spawn capability")


def tk_available():
    return bool(_MOLT_TK_AVAILABLE())


def tk_unavailable_message(operation):
    reason = _MOLT_TK_LAST_ERROR(None)
    if isinstance(reason, str) and reason:
        return reason
    return f"tkinter runtime unavailable ({operation})"


def require_tk_runtime(operation):
    if tk_available():
        return
    raise RuntimeError(tk_unavailable_message(operation))


# Frontend direct-call lowering may bind these underscore-prefixed symbols.
def _has_gui_capability():
    return has_gui_capability()


def _has_process_spawn_capability():
    return has_process_spawn_capability()


def _require_gui_capability():
    return require_gui_capability()


def _require_process_spawn_capability():
    return require_process_spawn_capability()


def _tk_available():
    return tk_available()


def _tk_unavailable_message(operation):
    return tk_unavailable_message(operation)


def _require_tk_runtime(operation):
    return require_tk_runtime(operation)
