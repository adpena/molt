"""Doctest stubs for Molt."""

from __future__ import annotations


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement doctest once eval/exec/compile are gated and supported.


def DocTestSuite(*_args, **_kwargs):
    raise RuntimeError(
        "MOLT_COMPAT_ERROR: doctest requires eval/exec/compile, which is not supported"
    )
