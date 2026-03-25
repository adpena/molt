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

_MOLT_SITE_HELP0 = _require_intrinsic("molt_site_help0")
_MOLT_SITE_HELP1 = _require_intrinsic("molt_site_help1")
_MOLT_SITE_QUITTER_CALL = _require_intrinsic("molt_site_quitter_call")


class _Helper:
    def __call__(
        self,
        *args: object,
        _help0_intrinsic=_MOLT_SITE_HELP0,
        _help1_intrinsic=_MOLT_SITE_HELP1,
        **kwds: object,
    ) -> None:
        del kwds
        if not args:
            _help0_intrinsic()
            return None
        _help1_intrinsic(args[0])
        return None


class _Printer:
    def __init__(self, name: str) -> None:
        self._name = name

    def __call__(self) -> None:
        _require_intrinsic(self._name)()
        return None


help = _Helper()

credits = _Printer("molt_site_credits")

copyright = _Printer("molt_site_copyright")

license = _Printer("molt_site_license")


class Quitter:
    def __init__(self, name: str) -> None:
        self._name = name

    def __repr__(self) -> str:
        return f"Use {self._name}() or Ctrl-D (i.e. EOF) to exit"

    def __call__(
        self, code: object = None, _quitter_call_intrinsic=_MOLT_SITE_QUITTER_CALL
    ) -> None:
        _quitter_call_intrinsic(code)
        return None


try:
    # Keep `inspect.signature` parity for callable instances.
    _Helper.__call__.__text_signature__ = "(self, *args, **kwds)"  # type: ignore[attr-defined]
    Quitter.__call__.__text_signature__ = "(self, code=None)"  # type: ignore[attr-defined]
except Exception:  # noqa: BLE001
    # Non-fatal: __text_signature__ is cosmetic (inspect.signature parity).
    # On WASM/micro builds, method objects may not support arbitrary attributes.
    pass


quit = Quitter("quit")
exit = Quitter("exit")

# Clean up module-level intrinsic references.  They are captured as
# default-argument values above, so the globals can be safely removed.
del _MOLT_SITE_HELP0, _MOLT_SITE_HELP1, _MOLT_SITE_QUITTER_CALL
