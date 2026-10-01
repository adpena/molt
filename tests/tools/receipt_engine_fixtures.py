"""Reuse one actual runtime observation in receipt-schema unit tests.

Production producers always observe afresh. These tests exercise immutable
serialized envelopes and producer custody; the canonical runtime-custody suite
owns OS census and file mutation checks. Reusing this real validated snapshot
avoids repeating its filesystem/native-image scan for every envelope fixture.
"""

from __future__ import annotations

import copy
from functools import cache

import pytest

from molt.python_runtime_identity import (
    capture_current_python_runtime,
    validate_python_runtime_identity,
)
from tools import receipt_toolchain


@cache
def observed_runtime_closure() -> dict[str, object]:
    observation = capture_current_python_runtime()
    validate_python_runtime_identity(observation)
    return observation


def install_observed_runtime(monkeypatch: pytest.MonkeyPatch) -> None:
    observation = observed_runtime_closure()
    monkeypatch.setattr(
        receipt_toolchain,
        "capture_current_python_runtime",
        lambda: copy.deepcopy(observation),
    )
