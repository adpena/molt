"""Minimal warnings_helper helpers for Molt (partial)."""

from __future__ import annotations

import warnings


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement
# full warnings_helper utilities (filter helpers, module-level predicates, state restoration).


class _CheckWarnings:
    def __init__(self) -> None:
        self._cm = warnings.catch_warnings(record=True)
        self._caught = None

    def __enter__(self):
        self._caught = self._cm.__enter__()
        warnings.simplefilter("always")
        return self._caught

    def __exit__(self, exc_type, exc, tb):
        return self._cm.__exit__(exc_type, exc, tb)


def check_warnings(*args, **kwargs):
    del args
    del kwargs
    return _CheckWarnings()


__all__ = ["check_warnings"]
