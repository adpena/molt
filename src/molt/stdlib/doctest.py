"""Doctest stubs for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


# TODO(stdlib-parity, owner:stdlib, milestone:SL3, priority:P3, status:planned): implement doctest once eval/exec/compile are gated and supported.


def DocTestSuite(*_args, **_kwargs):
    raise RuntimeError(
        "MOLT_COMPAT_ERROR: doctest requires eval/exec/compile, which is not supported"
    )
