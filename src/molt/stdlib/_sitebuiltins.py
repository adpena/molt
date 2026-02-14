"""Molt-native `_sitebuiltins`.

CPython installs `help`, `license`, `credits`, and `copyright` on `builtins`
via `site`. Molt compiled binaries do not ship host-Python tooling, but many
workflows expect these symbols to exist.

Non-negotiable: all behavior must lower into Rust intrinsics. This module is a
thin wrapper that exposes CPython-like surface types (`_Helper`, `_Printer`)
while delegating all observable behavior (printing) to Rust.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_SITE_HELP0 = _require_intrinsic("molt_site_help0", globals())
_MOLT_SITE_HELP1 = _require_intrinsic("molt_site_help1", globals())
_MOLT_SITE_CREDITS = _require_intrinsic("molt_site_credits", globals())
_MOLT_SITE_LICENSE = _require_intrinsic("molt_site_license", globals())
_MOLT_SITE_COPYRIGHT = _require_intrinsic("molt_site_copyright", globals())
_MOLT_SITE_QUITTER_CALL = _require_intrinsic("molt_site_quitter_call", globals())


class _Helper:
    # CPython: callable object, not a plain function.
    def __call__(self, *args: object, **kwds: object) -> None:
        # Keep output short and deterministic. Differential tests normalize this
        # to "nonempty output" rather than byte-for-byte pydoc parity.
        del kwds
        if not args:
            _MOLT_SITE_HELP0()
            return None
        _MOLT_SITE_HELP1(args[0])
        return None


class _Printer:
    def __init__(self, intrinsic) -> None:  # type: ignore[no-untyped-def]
        self._intrinsic = intrinsic

    def __call__(self) -> None:
        self._intrinsic()
        return None


help = _Helper()

credits = _Printer(_MOLT_SITE_CREDITS)

copyright = _Printer(_MOLT_SITE_COPYRIGHT)

license = _Printer(_MOLT_SITE_LICENSE)


class Quitter:
    # CPython: `quit` / `exit` are instances of this type.
    def __init__(self, name: str) -> None:
        self._name = name

    def __repr__(self) -> str:
        # Keep this deterministic and CPython-shaped enough for tooling.
        return f"Use {self._name}() or Ctrl-D (i.e. EOF) to exit"

    def __call__(self, code: object = None) -> None:
        _MOLT_SITE_QUITTER_CALL(code)
        return None


try:
    # Keep `inspect.signature` parity for callable instances.
    _Helper.__call__.__text_signature__ = "(self, *args, **kwds)"  # type: ignore[attr-defined]
    Quitter.__call__.__text_signature__ = "(self, code=None)"  # type: ignore[attr-defined]
except Exception as _exc:  # noqa: BLE001
    raise RuntimeError(
        "_sitebuiltins missing __text_signature__ support for inspect.signature parity"
    ) from _exc


quit = Quitter("quit")
exit = Quitter("exit")
