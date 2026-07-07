"""Fail-closed teeth for the todo_as_plan burndown.

Two formerly-partial stdlib surfaces are now honest:

* multiprocessing start methods (F8): fork/forkserver used to be advertised by
  get_all_start_methods() and then silently ran spawn semantics. Molt has no
  os.fork()/HAVE_SEND_HANDLE on any supported target, so they must not be
  advertised and get_context()/set_start_method() must fail closed with
  CPython's ValueError("cannot find context for %r").

* zlib.Decompress.unused_data (F9): it used to unconditionally return b"".
  It now returns the true bytes found past the end of the compressed stream,
  via the molt_zlib_decompressobj_unused_data intrinsic, distinct from
  unconsumed_tail (CPython zlib semantics).

These load the shipped Molt stdlib modules directly under CPython with a stubbed
_intrinsics module, so the pure-Python authority is exercised host-independently
without a full molt build. The runtime side of F9 (the Rust intrinsic that
tracks trailing bytes) is covered end-to-end by the differential test
tests/differential/stdlib/zlib_decompressobj_unused_data.py.
"""

from __future__ import annotations

import builtins
import importlib.util
import sys
import types
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"


def _install_intrinsics_stub(overrides: dict[str, object] | None = None) -> None:
    """Install a fake ``_intrinsics`` module.

    Any required intrinsic resolves to a placeholder callable unless an explicit
    override is supplied. Overrides let a test drive a specific intrinsic's
    return value (e.g. the unused_data bytes).
    """

    overrides = overrides or {}

    def _placeholder(name: str):
        # Module-level constant intrinsics (e.g. molt_zlib_def_buf_size) are
        # called at import time; return 0 so import succeeds. Behavioural
        # intrinsics under test are always supplied via ``overrides``.
        def _call(*_args, **_kwargs):
            return 0

        _call.__name__ = name
        return _call

    def _require_intrinsic(name: str, namespace: dict | None = None):
        value = overrides.get(name, _placeholder(name))
        if namespace is not None:
            namespace[name] = value
        return value

    module = types.ModuleType("_intrinsics")
    module.require_intrinsic = _require_intrinsic  # type: ignore[attr-defined]
    sys.modules["_intrinsics"] = module


def _load_stdlib_module(name: str, relpath: str) -> types.ModuleType:
    spec = importlib.util.spec_from_file_location(name, STDLIB_ROOT / relpath)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture()
def mp_core(monkeypatch: pytest.MonkeyPatch):
    _install_intrinsics_stub()
    # os.name must read as posix for the interesting branch (the Windows branch
    # already only advertised spawn); force posix so the test proves fork was
    # dropped even on the non-Windows path.
    monkeypatch.setattr("os.name", "posix", raising=False)
    mod = _load_stdlib_module(
        "molt_test__mp_core", "multiprocessing/_core.py"
    )
    yield mod
    sys.modules.pop("molt_test__mp_core", None)
    sys.modules.pop("_intrinsics", None)


def test_get_all_start_methods_only_spawn(mp_core) -> None:
    # fork/forkserver are unavailable on the Molt target and must not be
    # advertised, even on the posix path.
    assert mp_core.get_all_start_methods() == ["spawn"]


@pytest.mark.parametrize("method", ["fork", "forkserver"])
def test_get_context_fails_closed_on_unavailable_method(mp_core, method: str) -> None:
    # CPython raises ValueError("cannot find context for %r") for a method not in
    # its concrete-context table. fork/forkserver must raise, not silently return
    # a spawn Context.
    with pytest.raises(ValueError) as excinfo:
        mp_core.get_context(method)
    assert str(excinfo.value) == f"cannot find context for {method!r}"


def test_get_context_spawn_still_works(mp_core) -> None:
    ctx = mp_core.get_context("spawn")
    assert ctx.get_start_method() == "spawn"


@pytest.mark.parametrize("method", ["fork", "forkserver"])
def test_set_start_method_fails_closed_on_unavailable_method(
    mp_core, method: str
) -> None:
    with pytest.raises(ValueError) as excinfo:
        mp_core.set_start_method(method)
    assert str(excinfo.value) == f"cannot find context for {method!r}"


def test_set_start_method_spawn_ok(mp_core) -> None:
    mp_core.set_start_method("spawn")
    assert mp_core.get_start_method() == "spawn"


@pytest.fixture()
def zlib_mod():
    captured: dict[str, object] = {}

    def _unused_data(handle):
        # Echo whatever the handle carries; the test seeds it below.
        return captured.get("unused", b"")

    _install_intrinsics_stub(
        {
            "molt_zlib_decompressobj_unused_data": _unused_data,
        }
    )
    mod = _load_stdlib_module("molt_test__zlib", "zlib.py")
    mod._TEST_CAPTURED = captured  # type: ignore[attr-defined]
    yield mod
    sys.modules.pop("molt_test__zlib", None)
    sys.modules.pop("_intrinsics", None)


def test_unused_data_returns_intrinsic_bytes(zlib_mod) -> None:
    # Prove the property is wired to the real intrinsic and returns its bytes
    # verbatim (not a hardcoded b"").
    trailing = b"TRAILING-AFTER-STREAM"
    zlib_mod._TEST_CAPTURED["unused"] = trailing
    dec = zlib_mod.Decompress.__new__(zlib_mod.Decompress)
    dec._handle = 1234
    assert dec.unused_data == trailing


def test_unused_data_empty_when_no_trailing(zlib_mod) -> None:
    zlib_mod._TEST_CAPTURED["unused"] = b""
    dec = zlib_mod.Decompress.__new__(zlib_mod.Decompress)
    dec._handle = 1
    assert dec.unused_data == b""
