from __future__ import annotations

import importlib.util
import sys
import types
from collections.abc import Callable
from pathlib import Path

import pytest


SYS_PATH = Path(__file__).resolve().parents[1] / "src/molt/stdlib/sys.py"
SCALAR_INTRINSICS = frozenset(
    {"molt_sys_maxsize", "molt_sys_maxunicode", "molt_sys_byteorder"}
)


def _load_sys_shim(
    monkeypatch: pytest.MonkeyPatch,
    *,
    runtime_active: bool,
    scalar_intrinsics: dict[str, Callable[[], object]],
) -> tuple[types.ModuleType, list[str]]:
    require_calls: list[str] = []
    intrinsics = types.ModuleType("_intrinsics")

    def require_intrinsic(name: str, _namespace: object = None):
        require_calls.append(name)
        if name in scalar_intrinsics:
            return scalar_intrinsics[name]
        if runtime_active and name in SCALAR_INTRINSICS:
            raise RuntimeError(f"intrinsic unavailable: {name}")
        return lambda *_args, **_kwargs: None

    intrinsics.require_intrinsic = require_intrinsic  # type: ignore[attr-defined]
    intrinsics.runtime_active = lambda: runtime_active  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "_intrinsics", intrinsics)

    module_name = f"_molt_test_sys_scalar_metadata_{id(scalar_intrinsics)}"
    spec = importlib.util.spec_from_file_location(module_name, SYS_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, module_name, module)
    spec.loader.exec_module(module)
    return module, require_calls


def _valid_scalar_intrinsics() -> dict[str, Callable[[], object]]:
    return {
        "molt_sys_maxsize": lambda: 2**31 - 1,
        "molt_sys_maxunicode": lambda: 0x10FFFF,
        "molt_sys_byteorder": lambda: "big",
    }


def test_runtime_scalar_metadata_comes_from_required_intrinsics(monkeypatch):
    module, require_calls = _load_sys_shim(
        monkeypatch,
        runtime_active=True,
        scalar_intrinsics=_valid_scalar_intrinsics(),
    )

    module._init_metadata()

    assert module.maxsize == 2**31 - 1
    assert module.maxunicode == 0x10FFFF
    assert module.byteorder == "big"
    assert SCALAR_INTRINSICS.issubset(require_calls)


@pytest.mark.parametrize("missing", sorted(SCALAR_INTRINSICS))
def test_runtime_scalar_metadata_intrinsics_are_required(monkeypatch, missing):
    scalar_intrinsics = _valid_scalar_intrinsics()
    del scalar_intrinsics[missing]

    with pytest.raises(RuntimeError, match=f"intrinsic unavailable: {missing}"):
        _load_sys_shim(
            monkeypatch,
            runtime_active=True,
            scalar_intrinsics=scalar_intrinsics,
        )


@pytest.mark.parametrize(
    ("name", "value"),
    [
        ("molt_sys_maxsize", True),
        ("molt_sys_maxsize", 0),
        ("molt_sys_maxunicode", False),
        ("molt_sys_maxunicode", -1),
        ("molt_sys_maxunicode", 0x110000),
        ("molt_sys_byteorder", b"little"),
        ("molt_sys_byteorder", "middle"),
    ],
)
def test_runtime_scalar_metadata_rejects_invalid_values(monkeypatch, name, value):
    scalar_intrinsics = _valid_scalar_intrinsics()
    scalar_intrinsics[name] = lambda: value
    module, _ = _load_sys_shim(
        monkeypatch,
        runtime_active=True,
        scalar_intrinsics=scalar_intrinsics,
    )

    with pytest.raises(RuntimeError, match=f"{name} returned invalid value"):
        module._init_metadata()
    assert not any(
        metadata_name in module.__dict__
        for metadata_name in ("maxsize", "maxunicode", "byteorder")
    )


def test_runtime_scalar_metadata_preserves_intrinsic_exception(monkeypatch):
    failure = LookupError("target metadata unavailable")
    scalar_intrinsics = _valid_scalar_intrinsics()

    def fail() -> object:
        raise failure

    scalar_intrinsics["molt_sys_maxunicode"] = fail
    module, _ = _load_sys_shim(
        monkeypatch,
        runtime_active=True,
        scalar_intrinsics=scalar_intrinsics,
    )

    with pytest.raises(LookupError) as raised:
        module._init_metadata()
    assert raised.value is failure
    assert not any(
        metadata_name in module.__dict__
        for metadata_name in ("maxsize", "maxunicode", "byteorder")
    )


def test_inactive_reference_harness_uses_host_sys_metadata(monkeypatch):
    module, require_calls = _load_sys_shim(
        monkeypatch,
        runtime_active=False,
        scalar_intrinsics={},
    )

    module._init_metadata()

    assert module.maxsize == sys.maxsize
    assert module.maxunicode == sys.maxunicode
    assert module.byteorder == sys.byteorder
    assert SCALAR_INTRINSICS.isdisjoint(require_calls)
