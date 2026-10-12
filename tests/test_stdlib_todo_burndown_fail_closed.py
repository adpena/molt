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
def mp_core():
    _install_intrinsics_stub()
    mod = _load_stdlib_module("molt_test__mp_core", "multiprocessing/_core.py")
    yield mod
    sys.modules.pop("molt_test__mp_core", None)
    sys.modules.pop("_intrinsics", None)


def test_current_process_pid_is_live_and_not_queried_during_import(
    mp_core, monkeypatch
) -> None:
    import os

    calls = []
    values = iter([101, 202])

    def getpid():
        calls.append("getpid")
        return next(values)

    # Load the real package and API projection, preserving the host package after
    # the test. Only the host PID boundary and native core intrinsics are replaced.
    with monkeypatch.context() as patch:
        patch.setitem(sys.modules, "multiprocessing._core", mp_core)
        for name in ["multiprocessing", "multiprocessing._api_surface"]:
            patch.setitem(sys.modules, name, types.ModuleType(name))
        _load_stdlib_module(
            "multiprocessing._api_surface", "multiprocessing/_api_surface.py"
        )
        patch.setattr(os, "getpid", getpid)
        module = _load_stdlib_module("multiprocessing", "multiprocessing/__init__.py")
        assert calls == []
        process = module.current_process()
        assert module.current_process() is process
        assert calls == []
        assert process.pid == 101
        assert process.pid == 202
        assert calls == ["getpid", "getpid"]
        with pytest.raises(AttributeError):
            process.pid = 303
        denied = PermissionError("PID denied")

        def reject_pid():
            raise denied

        patch.setattr(os, "getpid", reject_pid)
        with pytest.raises(PermissionError) as error:
            _ = process.pid
        assert error.value is denied


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
def mp_spawn(mp_core, monkeypatch):
    calls = []

    def unused(*args, **kwargs):
        pytest.fail(f"unexpected spawn boundary: {args!r} {kwargs!r}")

    # Every host boundary reachable before publication must be explicitly
    # selected below. The source resolver/start/context methods stay real.
    for name in (
        "_MOLT_REQUIRE_OPERATION",
        "_MOLT_PROCESS_SPAWN",
        "_MOLT_PROCESS_STDIN",
        "_MOLT_PROCESS_STDOUT",
        "_MOLT_ENV_GET",
        "_MOLT_ENV_SNAPSHOT",
        "_MOLT_MP_CODEC_DUMPS",
        "_MOLT_STREAM_SEND",
        "_MOLT_PENDING",
    ):
        monkeypatch.setattr(mp_core, name, unused)
    return mp_core, calls


def _spawn_from_surface(module, surface):
    if surface == "process":
        process = module.Process()
        process.start()
        return process
    if surface == "worker":
        return module._Worker(None, (), None, "spawn")
    return module.Pool(processes=1)


@pytest.mark.parametrize("surface", ["process", "worker", "pool"])
@pytest.mark.parametrize(
    "missing", ["process.exec, env.read", "process.exec", "env.read"]
)
def test_spawn_denial_precedes_environment_or_native_work(
    mp_spawn, monkeypatch, surface, missing
):
    module, calls = mp_spawn
    denial = PermissionError(
        f"missing [{missing}] capabilities for 'multiprocessing.spawn'"
    )

    def require(operation):
        calls.append(operation)
        raise denial

    monkeypatch.setattr(module, "_MOLT_REQUIRE_OPERATION", require)
    with pytest.raises(PermissionError) as caught:
        _spawn_from_surface(module, surface)
    assert caught.value is denial
    assert calls == ["multiprocessing.spawn"]


def _admit_spawn_environment(module, calls, monkeypatch):
    def require(operation):
        calls.append(("require", operation))

    def get_env(key, default):
        calls.append(("env_get", key))
        return default

    def snapshot():
        calls.append(("snapshot",))
        return {"SELECTED_ENV": "retained"}

    monkeypatch.setattr(module, "_MOLT_REQUIRE_OPERATION", require)
    monkeypatch.setattr(module, "_MOLT_ENV_GET", get_env)
    monkeypatch.setattr(module, "_MOLT_ENV_SNAPSHOT", snapshot)


@pytest.mark.parametrize("surface", ["process", "worker", "pool"])
def test_spawn_admission_precedes_environment_and_one_real_transport_setup(
    mp_spawn, monkeypatch, surface
):
    module, calls = mp_spawn
    _admit_spawn_environment(module, calls, monkeypatch)

    def spawn(args, env, cwd, stdin, stdout, stderr):
        calls.append(("spawn",))
        assert env["SELECTED_ENV"] == "retained"
        assert env["MOLT_MP_START_METHOD"] == "spawn"
        assert env["MOLT_MP_SPAWN"] == "1"
        assert env["MOLT_ENTRY_MODULE"] == "multiprocessing.spawn"
        assert (stdin, stdout, stderr) == (1, 1, 0)
        return 41

    frames = []
    monkeypatch.setattr(module, "_MOLT_PROCESS_SPAWN", spawn)
    monkeypatch.setattr(
        module,
        "_MOLT_PROCESS_STDIN",
        lambda handle: 51 if handle == 41 else pytest.fail("wrong process"),
    )
    monkeypatch.setattr(
        module,
        "_MOLT_PROCESS_STDOUT",
        lambda handle: 52 if handle == 41 else pytest.fail("wrong process"),
    )
    monkeypatch.setattr(module, "_MOLT_MP_CODEC_DUMPS", lambda message: b"encoded")
    monkeypatch.setattr(
        module, "_MOLT_STREAM_SEND", lambda stream, data: frames.append((stream, data))
    )
    monkeypatch.setattr(module, "_MOLT_PENDING", object)
    owner = _spawn_from_surface(module, surface)
    assert calls[0] == ("require", "multiprocessing.spawn")
    assert calls.count(("require", "multiprocessing.spawn")) == 1
    assert calls.count(("spawn",)) == 1
    assert calls.index(("snapshot",)) < calls.index(("spawn",))
    assert frames == [(51, b"\x00\x00\x00\x0b\xffMP1encoded")]
    process = owner._workers[0] if surface == "pool" else owner
    assert process._handle == 41
    assert process._hub is not None and process._transport is not None
    if surface == "process":
        assert process._started
        before = list(calls)
        with pytest.raises(RuntimeError, match="already started"):
            process.start()
        assert calls == before


@pytest.mark.parametrize("failure", ["denied", "missing", "runtime", "null"])
def test_process_start_failure_never_runs_target_inline_or_marks_started(
    mp_spawn, monkeypatch, failure
):
    module, calls = mp_spawn
    _admit_spawn_environment(module, calls, monkeypatch)
    ran = []
    process = module.Process(target=lambda: ran.append("inline"))
    selected = RuntimeError("native spawn unavailable")
    expected = selected
    if failure == "denied":
        expected = PermissionError("complete selected denial")

        def require(operation):
            assert operation == "multiprocessing.spawn"
            raise expected

        monkeypatch.setattr(module, "_MOLT_REQUIRE_OPERATION", require)
    elif failure == "missing":
        monkeypatch.setattr(module, "_MOLT_PROCESS_SPAWN", None)
    elif failure == "null":
        monkeypatch.setattr(module, "_MOLT_PROCESS_SPAWN", lambda *args: None)
    else:

        def spawn(*args):
            raise selected

        monkeypatch.setattr(module, "_MOLT_PROCESS_SPAWN", spawn)
    with pytest.raises((RuntimeError, PermissionError)) as caught:
        process.start()
    if failure in {"runtime", "denied"}:
        assert caught.value is expected
    elif failure == "missing":
        assert str(caught.value) == "Missing intrinsic: molt_process_spawn"
    else:
        assert str(caught.value) == "process spawn failed"
    assert ran == []
    assert process._started is False
    assert process._handle is None and process._exitcode is None
    assert process._hub is None and process._transport is None
    # The same instance can make another admitted attempt after a failed start.
    monkeypatch.setattr(module, "_MOLT_REQUIRE_OPERATION", lambda operation: None)
    monkeypatch.setattr(module, "_MOLT_PROCESS_SPAWN", lambda *args: None)
    with pytest.raises(RuntimeError, match="^process spawn failed$"):
        process.start()
    assert not process._started and ran == []


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
