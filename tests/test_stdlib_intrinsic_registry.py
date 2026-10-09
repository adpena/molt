"""The shared intrinsic registry installs the real resolver and real anchors."""

from __future__ import annotations

import builtins
import sys

import pytest

from molt._intrinsic_symbols import INTRINSIC_SYMBOL_NAMES
from tests.stdlib_intrinsic_registry import RESOLVER, anchors, intrinsic_registry


def test_anchors_are_registered_runtime_intrinsics() -> None:
    names = anchors()
    assert {
        "molt_capabilities_has",
        "molt_import_smoke_runtime_ready",
        "molt_stdlib_probe",
    } <= names.keys()
    assert names.keys() <= INTRINSIC_SYMBOL_NAMES.keys()
    assert all(
        anchor() is True
        for name, anchor in names.items()
        if name != "molt_capabilities_has"
    )
    assert names["molt_capabilities_has"]("fs.read") is True


def test_registry_resolves_through_the_real_resolver_and_restores() -> None:
    before = sys.modules.get("_intrinsics")
    behavior = {"molt_example_behavior": len}
    with intrinsic_registry(behavior) as resolver:
        assert resolver.__file__ == str(RESOLVER)
        assert sys.modules["_intrinsics"] is resolver
        assert resolver.require_intrinsic("molt_example_behavior") is len
        assert resolver.require_intrinsic("molt_import_smoke_runtime_ready")() is True
        with pytest.raises(RuntimeError, match="intrinsic unavailable: molt_absent"):
            resolver.require_intrinsic("molt_absent")
    assert sys.modules.get("_intrinsics") is before
    assert not hasattr(builtins, "_molt_intrinsics_strict")


def test_registry_without_anchors_holds_only_the_given_behavior() -> None:
    with intrinsic_registry({}, with_anchors=False) as resolver:
        with pytest.raises(
            RuntimeError, match="intrinsic unavailable: molt_capabilities_has"
        ):
            resolver.require_intrinsic("molt_capabilities_has")
