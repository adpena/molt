"""Shared tkinter capability/runtime gating helpers."""

import _tkinter as _tk_runtime

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_MOLT_TK_AVAILABLE = _require_intrinsic("molt_tk_available")
_MOLT_TK_LAST_ERROR = _require_intrinsic("molt_tk_last_error")


# Keep the actual _tkinter wrapper function: these callables accept both
# TkappType and raw interpreter handles through the shared _unwrap_app law.
# Missing or replaced non-callable providers fail at acquisition.
def _require_tk_callable(attr):
    candidate = getattr(_tk_runtime, attr, None)
    if not callable(candidate):
        raise RuntimeError(f"tkinter runtime callable unavailable: {attr}")
    return candidate


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


globals().pop("_require_intrinsic", None)
