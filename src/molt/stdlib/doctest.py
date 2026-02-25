"""Doctest stubs for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


# Policy-deferred: doctest parity that depends on dynamic execution (`eval`/`exec`/`compile`) remains intentionally unsupported for now; revisit only behind explicit capability gating after utility analysis, performance evidence, and explicit user approval.


def DocTestSuite(*_args, **_kwargs):
    raise RuntimeError(
        "MOLT_COMPAT_ERROR: doctest requires eval/exec/compile, which is not supported"
    )
