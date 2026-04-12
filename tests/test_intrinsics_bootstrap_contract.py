from __future__ import annotations

import importlib
import importlib.util
from pathlib import Path
import sys
import types

import pytest
import builtins


ROOT = Path(__file__).resolve().parents[1]
ROOT_INTRINSICS_PATH = ROOT / "src" / "_intrinsics.py"
STDLIB_INTRINSICS_PATH = ROOT / "src" / "molt" / "stdlib" / "_intrinsics.py"


def _load_intrinsics(path: Path, module_name: str) -> types.ModuleType:
    spec = importlib.util.spec_from_file_location(module_name, path)
    assert spec is not None
    loader = spec.loader
    assert loader is not None
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def _load_root_intrinsics(module_name: str) -> types.ModuleType:
    return _load_intrinsics(ROOT_INTRINSICS_PATH, module_name)


def _load_stdlib_intrinsics(module_name: str) -> types.ModuleType:
    return _load_intrinsics(STDLIB_INTRINSICS_PATH, module_name)


@pytest.fixture(autouse=True)
def _clear_runtime_intrinsics(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delattr(builtins, "_molt_intrinsic_lookup", raising=False)
    monkeypatch.delattr(builtins, "_molt_intrinsics", raising=False)
    monkeypatch.delattr(builtins, "_molt_runtime", raising=False)
    monkeypatch.delattr(builtins, "_molt_intrinsics_strict", raising=False)


def test_stdlib_intrinsics_does_not_use_namespace_binding_without_helper() -> None:
    loader = _load_stdlib_intrinsics("_molt_test_intrinsics_ns")
    namespace_fn = lambda: "namespace"  # noqa: E731
    namespace = {"molt_probe": namespace_fn}
    with pytest.raises(RuntimeError, match="runtime inactive"):
        loader.require_intrinsic("molt_probe", namespace)


def test_stdlib_intrinsics_runtime_mode_requires_helper_not_registry() -> None:
    loader = _load_stdlib_intrinsics("_molt_test_intrinsics_runtime")
    runtime_fn = lambda: "runtime"  # noqa: E731
    loader.__dict__["_molt_intrinsics"] = {"molt_probe": runtime_fn}
    loader.__dict__["_molt_runtime"] = True
    with pytest.raises(RuntimeError, match="intrinsic unavailable"):
        loader.require_intrinsic("molt_probe", None)


def test_stdlib_intrinsics_does_not_fallback_to_sys_modules_registry() -> None:
    loader = _load_stdlib_intrinsics("_molt_test_intrinsics_no_sys_modules_fallback")
    runtime_fn = lambda: "runtime"  # noqa: E731
    loader.__dict__["_molt_intrinsics"] = {"molt_probe": runtime_fn}
    with pytest.raises(RuntimeError, match="runtime inactive"):
        loader.require_intrinsic("molt_probe", None)


def test_stdlib_intrinsics_uses_runtime_lookup_helper() -> None:
    loader = _load_stdlib_intrinsics("_molt_test_intrinsics_runtime_helper")
    runtime_fn = lambda: "runtime"  # noqa: E731

    def _lookup(name: str):  # type: ignore[no-untyped-def]
        if name == "molt_probe":
            return runtime_fn
        return None

    loader.__dict__["_molt_intrinsic_lookup"] = _lookup
    assert loader.require_intrinsic("molt_probe", None) is runtime_fn


def test_root_intrinsics_uses_runtime_lookup_helper() -> None:
    loader = _load_root_intrinsics("_molt_test_root_intrinsics_runtime_helper")
    runtime_fn = lambda: "runtime"  # noqa: E731

    def _lookup(name: str):  # type: ignore[no-untyped-def]
        if name == "molt_probe":
            return runtime_fn
        return None

    loader.__dict__["_molt_intrinsic_lookup"] = _lookup
    assert loader.require_intrinsic("molt_probe", None) is runtime_fn


def test_root_intrinsics_uses_builtins_registry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    loader = _load_root_intrinsics("_molt_test_root_intrinsics_builtins_registry")
    runtime_fn = lambda: "runtime"  # noqa: E731

    monkeypatch.setattr(
        builtins,
        "_molt_intrinsics",
        {"molt_probe": runtime_fn},
        raising=False,
    )
    assert loader.require_intrinsic("molt_probe", None) is runtime_fn


def test_stdlib_intrinsics_uses_builtins_runtime_lookup_helper(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    loader = _load_stdlib_intrinsics("_molt_test_intrinsics_builtins_runtime_helper")
    runtime_fn = lambda: "runtime"  # noqa: E731

    def _lookup(name: str):  # type: ignore[no-untyped-def]
        if name == "molt_probe":
            return runtime_fn
        return None

    monkeypatch.setattr(builtins, "_molt_intrinsic_lookup", _lookup, raising=False)
    assert loader.require_intrinsic("molt_probe", None) is runtime_fn


def test_stdlib_intrinsics_ignores_sys_modules_runtime_helper(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    loader = _load_stdlib_intrinsics("_molt_test_intrinsics_runtime_helper_sys_modules")
    helper = lambda _name: lambda: "runtime"  # noqa: E731
    monkeypatch.setitem(
        sys.modules,
        "builtins",
        types.SimpleNamespace(_molt_intrinsic_lookup=helper),
    )
    with pytest.raises(RuntimeError, match="runtime inactive"):
        loader.require_intrinsic("molt_probe", None)


def test_stdlib_intrinsics_reports_runtime_inactive_when_unavailable() -> None:
    loader = _load_stdlib_intrinsics("_molt_test_intrinsics_missing")
    with pytest.raises(RuntimeError, match="runtime inactive"):
        loader.require_intrinsic("molt_not_present", {})


def test_package_intrinsics_delegate_to_stdlib_loader(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[tuple[str, str]] = []
    fake_loader = types.ModuleType("_intrinsics")

    def _load(name: str, namespace=None):  # type: ignore[no-untyped-def]
        calls.append(("load", name))
        if namespace is None:
            return None
        return ("load", name, namespace)

    def _require(name: str, namespace=None):  # type: ignore[no-untyped-def]
        calls.append(("require", name))
        return ("require", name, namespace)

    def _runtime_active() -> bool:
        calls.append(("runtime_active", ""))
        return True

    fake_loader.load_intrinsic = _load  # type: ignore[attr-defined]
    fake_loader.require_intrinsic = _require  # type: ignore[attr-defined]
    fake_loader.runtime_active = _runtime_active  # type: ignore[attr-defined]
    import molt.intrinsics as package_intrinsics

    # Save original _loader so we can restore after the test.
    original_loader = package_intrinsics._loader  # type: ignore[attr-defined]

    monkeypatch.setitem(sys.modules, "_intrinsics", fake_loader)
    package_intrinsics = importlib.reload(package_intrinsics)

    try:
        namespace = {"k": "v"}
        assert package_intrinsics.runtime_active() is True
        assert package_intrinsics.load("molt_probe", namespace) == (
            "load",
            "molt_probe",
            namespace,
        )
        assert package_intrinsics.load_intrinsic("molt_probe", namespace) == (
            "load",
            "molt_probe",
            namespace,
        )
        assert package_intrinsics.require("molt_need", namespace) == (
            "require",
            "molt_need",
            namespace,
        )
        assert package_intrinsics.require_intrinsic("molt_need", namespace) == (
            "require",
            "molt_need",
            namespace,
        )
        assert ("runtime_active", "") in calls
        assert ("load", "molt_probe") in calls
        assert ("require", "molt_need") in calls
    finally:
        # Restore the real _loader so later tests using molt.intrinsics
        # (e.g. capabilities) don't see the fake.
        package_intrinsics._loader = original_loader  # type: ignore[attr-defined]


def test_capabilities_use_package_intrinsics_loader(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import molt.capabilities as capabilities

    captured: list[tuple[str, object]] = []

    def _fake_require(name: str, namespace=None):  # type: ignore[no-untyped-def]
        captured.append((name, namespace))
        if name == "molt_env_get":
            return lambda _key, _default="": "net,fs"
        if name == "molt_capabilities_trusted":
            return lambda: True
        if name == "molt_capabilities_has":
            return lambda cap: cap == "net"
        if name == "molt_capabilities_require":
            return lambda _cap: None
        raise AssertionError(f"unexpected intrinsic lookup: {name}")

    monkeypatch.setattr(capabilities._intrinsics, "require", _fake_require)
    assert capabilities.capabilities() == {"net", "fs"}
    assert capabilities.trusted() is True
    assert capabilities.has("net") is True
    capabilities.require("net")
    first_name, first_namespace = captured[0]
    assert first_name == "molt_env_get"
    assert first_namespace is capabilities.__dict__
