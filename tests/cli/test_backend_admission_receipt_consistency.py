from __future__ import annotations

from contextlib import contextmanager
import json
from pathlib import Path

import pytest

from molt.cli import backend_binary, build_locks, runtime_fingerprints
from molt.cli.static_archive_identity import artifact_content_identity
from tests.cli.test_cli_backend_prewarm import (
    _dispatch,
    _fake_backend_toolchain,
    _isolated_molt_root,
)


@pytest.mark.parametrize("acquisition", [1, 2])
def test_prewarm_lock_failure_is_structured(acquisition, tmp_path, monkeypatch, capsys):
    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    original = backend_binary._build_lock
    calls = 0

    @contextmanager
    def locked(*args, **kwargs):
        nonlocal calls
        calls += 1
        if calls == acquisition:
            raise RuntimeError("bounded lock timeout")
        with original(*args, **kwargs):
            yield

    monkeypatch.setattr(backend_binary, "_build_lock", locked)
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 2
    captured = capsys.readouterr()
    payload = json.loads(captured.out)
    assert payload["status"] == "error"
    assert "bounded lock timeout" in captured.err
    if acquisition == 1:
        assert payload["data"]["failure"]["phase"] == "backend_build_lock"
    else:
        assert payload["data"]["failure"]["phase"] == "backend_receipt_lock"


def test_cargo_spawn_failure_is_structured(tmp_path, monkeypatch, capsys):
    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)

    def unavailable(*args, **kwargs):
        raise FileNotFoundError("cargo executable missing")

    monkeypatch.setattr(backend_binary, "_run_cargo_with_sccache_retry", unavailable)
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 2
    captured = capsys.readouterr()
    payload = json.loads(captured.out)
    assert payload["data"]["failure"]["phase"] == "backend_cargo_build"
    assert "cargo executable missing" in captured.err


@pytest.mark.parametrize("override,expected", [(None, 1200), ("0.2", 0.2)])
def test_backend_lock_wait_covers_cold_build_and_preserves_override(
    override, expected, tmp_path, monkeypatch
):
    if override is None:
        monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    else:
        monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", override)
    seen = []

    def acquire(path, **kwargs):
        seen.append(kwargs["timeout_s"])
        return object()

    monkeypatch.setattr(build_locks, "_acquire_file_lock", acquire)
    monkeypatch.setattr(build_locks, "_release_file_lock", lambda handle: None)
    with backend_binary._backend_admission_lock(
        tmp_path, "release", cargo_timeout=1200
    ):
        pass
    assert seen == [expected]


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_unavailable_rustc_preserves_admitted_receipt_cache_identity(
    target, tmp_path, monkeypatch, capsys
):
    _isolated_molt_root(tmp_path, monkeypatch)
    cargo, probes = _fake_backend_toolchain(monkeypatch)
    argv = ["internal-backend-build", "--target", target, "--json"]
    assert _dispatch(argv) == 0
    before = json.loads(capsys.readouterr().out)["data"]
    original = backend_binary._backend_fingerprint

    def no_rustc(*args, **kwargs):
        fingerprint = original(*args, **kwargs)
        fingerprint["rustc"] = None
        return fingerprint

    monkeypatch.setattr(backend_binary, "_backend_fingerprint", no_rustc)
    assert _dispatch(argv) == 0
    after = json.loads(capsys.readouterr().out)["data"]
    assert after["compiler"]["fingerprint"] == before["compiler"]["fingerprint"]
    assert after["receipts"]["source_content"]["rustc"] == "rustc-fixture"
    assert len(cargo) == len(probes) == 1


@pytest.mark.parametrize("missing_rustc", [False, True])
def test_identical_feature_alias_reuses_transferred_probe(
    missing_rustc, tmp_path, monkeypatch
):
    _isolated_molt_root(tmp_path, monkeypatch)
    cargo, probes = _fake_backend_toolchain(monkeypatch)
    argv = ["internal-backend-build", "--target", "wasm", "--json"]
    assert _dispatch(argv) == 0
    alias = Path(probes[0][0])
    alias.unlink()
    if missing_rustc:
        original = backend_binary._backend_fingerprint

        def no_rustc(*args, **kwargs):
            return {**original(*args, **kwargs), "rustc": None}

        monkeypatch.setattr(backend_binary, "_backend_fingerprint", no_rustc)
    assert _dispatch(argv) == 0
    assert len(cargo) == 1
    assert len(probes) == 2  # one canonical probe, no duplicate alias probe
    assert alias.exists()


def test_refresh_write_failure_is_nonfatal_but_invalid_identity_is_not(
    tmp_path, monkeypatch
):
    artifact = tmp_path / "compiler"
    artifact.write_bytes(b"compiler")
    receipt = tmp_path / "receipt.json"
    source = {"hash": "a" * 64, "rustc": "rustc", "inputs_digest": "b" * 64}
    runtime_fingerprints._write_runtime_fingerprint(
        receipt,
        {**source, "artifact_content_identity": artifact_content_identity(artifact)},
    )
    before = receipt.read_bytes()

    def unwritable(*args, **kwargs):
        raise PermissionError("metadata directory read-only")

    monkeypatch.setattr(runtime_fingerprints, "_atomic_write_json", unwritable)
    runtime_fingerprints._refresh_runtime_fingerprint_metadata(
        receipt, {**source, "inputs_digest": "c" * 64}
    )
    assert receipt.read_bytes() == before
    with pytest.raises(ValueError, match="cannot change"):
        runtime_fingerprints._refresh_runtime_fingerprint_metadata(
            receipt, {**source, "hash": "d" * 64}
        )
    receipt.unlink()
    with pytest.raises(ValueError, match="lost custody"):
        runtime_fingerprints._refresh_runtime_fingerprint_metadata(receipt, source)


@pytest.mark.parametrize("suffix", [".exe", ".wasm", ".a"])
def test_hydration_preserves_matched_toolchain_coordinates(suffix, tmp_path):
    from molt.cli.artifact_state import _maybe_hydrate_artifact_from_canonical_target
    from tests.cli.native_link_test_support import static_archive_bytes
    from tests.native_artifact_fixtures import native_relocatable_object

    image = b"\0asm\x01\0\0\0" if suffix == ".wasm" else b"compiler"
    if suffix == ".a":
        image = static_archive_bytes(native_relocatable_object())
    candidate = tmp_path / ("canonical" + suffix)
    candidate.write_bytes(image)
    receipt = tmp_path / "canonical.json"
    fingerprint = {
        "hash": "a" * 64,
        "rustc": "known-toolchain",
        "inputs_digest": "b" * 64,
    }
    runtime_fingerprints._write_runtime_fingerprint(
        receipt, fingerprint, artifact=candidate
    )
    hydrated = tmp_path / ("session" + suffix)
    hydrated_receipt = tmp_path / "session.json"
    assert _maybe_hydrate_artifact_from_canonical_target(
        artifact=hydrated,
        fingerprint={**fingerprint, "rustc": None},
        fingerprint_path=hydrated_receipt,
        candidate_artifact=candidate,
        candidate_fingerprint_path=receipt,
        require_artifact_digest=True,
    )
    assert (
        runtime_fingerprints._read_runtime_fingerprint(hydrated_receipt)["rustc"]
        == "known-toolchain"
    )
    assert hydrated.read_bytes() == image
    assert not _maybe_hydrate_artifact_from_canonical_target(
        artifact=hydrated,
        fingerprint={**fingerprint, "rustc": "different-toolchain"},
        fingerprint_path=hydrated_receipt,
        candidate_artifact=candidate,
        candidate_fingerprint_path=receipt,
        require_artifact_digest=True,
    )


@pytest.mark.parametrize("bad_capacity", ["abc", "1000000000"])
def test_real_cargo_capacity_rejection_preserves_prewarm_json(
    bad_capacity, tmp_path, monkeypatch, capsys
):
    from molt.cli import cargo_execution

    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    # Keep Cargo admission real: capacity must fail before any external launch.
    monkeypatch.setattr(
        backend_binary,
        "_run_cargo_with_sccache_retry",
        cargo_execution._run_cargo_with_sccache_retry,
    )
    monkeypatch.setenv("MOLT_DISK_GUARD_HIGH_WATER_GB", bad_capacity)

    def forbidden(*args, **kwargs):
        raise AssertionError("capacity rejection must precede process launch")

    monkeypatch.setattr(cargo_execution, "_run_completed_command", forbidden)
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == "backend_cargo_build"
    assert "diagnostic=" in payload["errors"][0]


@pytest.mark.parametrize("failure_kind", ["capacity", "spawn"])
def test_real_feature_rebuild_rejection_preserves_json_and_timing(
    failure_kind, tmp_path, monkeypatch, capsys
):
    import subprocess
    from molt.cli import cargo_execution

    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    monkeypatch.setattr(
        backend_binary,
        "_run_cargo_with_sccache_retry",
        cargo_execution._run_cargo_with_sccache_retry,
    )
    external_calls = []

    def external(cmd, **kwargs):
        external_calls.append(cmd)
        if len(external_calls) > 1:
            raise FileNotFoundError("feature rebuild cargo vanished")
        output = (
            Path(kwargs["env"]["CARGO_TARGET_DIR"])
            / "release"
            / ("molt-backend.exe" if __import__("os").name == "nt" else "molt-backend")
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(b"backend executable")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    def mismatch(cmd, **kwargs):
        if failure_kind == "capacity":
            monkeypatch.setenv("MOLT_DISK_GUARD_HIGH_WATER_GB", "abc")
            # The backend captured build_env before its first Cargo attempt;
            # update that same environment at the actual external boundary.
            captured_env["MOLT_DISK_GUARD_HIGH_WATER_GB"] = "abc"
        return subprocess.CompletedProcess(cmd, 1, b"", b"feature mismatch")

    captured_env = {}
    original_attempt = cargo_execution._run_cargo_attempt

    def attempt(*args, **kwargs):
        nonlocal captured_env
        captured_env = kwargs["env"]
        return original_attempt(*args, **kwargs)

    monkeypatch.setattr(cargo_execution, "_run_cargo_attempt", attempt)
    monkeypatch.setattr(cargo_execution, "_run_completed_command", external)
    monkeypatch.setattr(
        backend_binary, "_run_subprocess_captured_to_tempfiles", mismatch
    )
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == "backend_feature_rebuild"
    assert payload["data"]["stage_timings_ms"]["backend_binary_feature_rebuild"] >= 0
    assert len(external_calls) == (1 if failure_kind == "capacity" else 2)


def test_alias_copy_rejects_bytes_changed_after_successful_probe(
    tmp_path, monkeypatch, capsys
):
    _isolated_molt_root(tmp_path, monkeypatch)
    cargo, probes = _fake_backend_toolchain(monkeypatch)
    argv = ["internal-backend-build", "--target", "wasm", "--json"]
    assert _dispatch(argv) == 0
    initial = json.loads(capsys.readouterr().out)["data"]
    alias = Path(initial["compiler"]["path"])
    alias_receipt = Path(initial["receipts"]["source_content"]["path"])
    alias.unlink()
    alias_receipt.unlink()
    original = backend_binary._atomic_copy_file

    def changed_copy(source, destination, **kwargs):
        source.write_bytes(b"not the compiler that was probed")
        return original(source, destination, **kwargs)

    monkeypatch.setattr(backend_binary, "_atomic_copy_file", changed_copy)
    assert _dispatch(argv) == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == "backend_alias_publication"
    assert not alias_receipt.exists()
    assert len(cargo) == 1


def test_build_lock_real_timeout_does_not_mask_inner_failure(tmp_path, monkeypatch):
    from molt.file_locks import _acquire_file_lock, _release_file_lock

    monkeypatch.setattr(build_locks, "_build_state_root", lambda root: tmp_path)
    lock_path = tmp_path / "build_locks" / "held.lock"
    handle = _acquire_file_lock(
        lock_path, timeout_s=1, timeout_message="initial lock unavailable"
    )
    monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", "0.02")
    try:
        with pytest.raises(
            build_locks.BuildLockAcquisitionError, match="Timed out waiting"
        ):
            with build_locks._build_lock(tmp_path, "held"):
                pytest.fail("held lock must not grant admission")
    finally:
        _release_file_lock(handle)
    with pytest.raises(RuntimeError, match="inner work failed") as raised:
        with build_locks._build_lock(tmp_path, "held"):
            raise RuntimeError("inner work failed")
    assert not isinstance(raised.value, build_locks.BuildLockAcquisitionError)


@pytest.mark.parametrize(
    "phase", ["backend_daemon_start_lock", "backend_daemon_restart_lock"]
)
def test_daemon_lock_failures_share_framing_and_timing(
    phase, tmp_path, monkeypatch, capsys
):
    from molt.cli import backend_compile

    def unavailable(*args, **kwargs):
        raise RuntimeError("daemon lock timed out")

    monkeypatch.setattr(build_locks, "_acquire_file_lock", unavailable)
    ready, error = backend_compile._start_backend_daemon_under_lock(
        tmp_path / "backend",
        tmp_path / "daemon.sock",
        cargo_profile="release",
        project_root=tmp_path,
        target_triple=None,
        config_digest="fixture",
        startup_timeout=1.0,
        json_output=True,
        warnings=[],
        backend_env={},
        phase=phase,
    )
    assert not ready and error is not None
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == phase
    assert payload["data"]["stage_timings_ms"][phase] >= 0


def test_shared_hydration_copy_race_cannot_publish_receipt(tmp_path, monkeypatch):
    from molt.cli import artifact_state

    artifact = tmp_path / "canonical.wasm"
    artifact.write_bytes(b"\0asm\x01\0\0\0")
    fingerprint = {"hash": "a" * 64, "rustc": "rustc", "inputs_digest": "b" * 64}
    receipt = tmp_path / "canonical.json"
    runtime_fingerprints._write_runtime_fingerprint(
        receipt, fingerprint, artifact=artifact
    )
    destination = tmp_path / "session.wasm"
    destination_receipt = tmp_path / "session.json"
    original = artifact_state._atomic_copy_file

    def mutate(source, target, **kwargs):
        source.write_bytes(b"\0asm\x01\0\0\0\x00\x02\x01x")
        return original(source, target, **kwargs)

    monkeypatch.setattr(artifact_state, "_atomic_copy_file", mutate)
    assert not artifact_state._maybe_hydrate_artifact_from_canonical_target(
        artifact=destination,
        fingerprint=fingerprint,
        fingerprint_path=destination_receipt,
        candidate_artifact=artifact,
        candidate_fingerprint_path=receipt,
        require_artifact_digest=True,
    )
    assert not destination_receipt.exists()


@pytest.mark.parametrize("inside_work", [False, True])
def test_wasm_member_lock_boundary_distinguishes_acquisition_from_work(
    inside_work, tmp_path, monkeypatch, capsys
):
    from molt.cli import runtime_wasm_build, build_locks
    from molt.cli.runtime_wasm_build_spec import _RuntimeWasmBuildSpec
    from molt.cli.runtime_artifact_selection import RUNTIME_CDYLIB_ARTIFACTS

    spec = _RuntimeWasmBuildSpec(
        requested_cargo_profile="dev-fast",
        cargo_profile="dev-fast",
        profile_dir="dev-fast",
        incremental_enabled=False,
        env={},
        artifact_selection=RUNTIME_CDYLIB_ARTIFACTS,
        runtime_exports="",
        link_flags="",
        cargo_rustflags="",
        fingerprint_rustflags="",
        no_default_features=True,
        wasm_cargo_features=(),
        fingerprint_features=(),
        fingerprint_path=tmp_path / "fingerprint",
        target_root=tmp_path / "target",
        stored_fingerprint=None,
        fingerprint={"hash": "a" * 64},
        staticlib_fingerprint={"hash": "b" * 64},
    )
    phases = []
    monkeypatch.setattr(
        runtime_wasm_build,
        "_record_runtime_wasm_build_phase",
        lambda *args, **kwargs: phases.append((args, kwargs)),
    )

    def broken(*args, **kwargs):
        raise RuntimeError("WASM lock boundary failure")

    if inside_work:
        monkeypatch.setattr(runtime_wasm_build, "_reuse_target_runtime_wasm", broken)
    else:
        monkeypatch.setattr(build_locks, "_acquire_file_lock", broken)

    def run():
        return runtime_wasm_build._materialize_runtime_wasm_member_from_target(
            tmp_path / "runtime.wasm",
            reloc=False,
            json_output=True,
            cargo_timeout=1,
            project_root=tmp_path,
            required_exports=None,
            resolved_modules=None,
            spec=spec,
        )

    if inside_work:
        with pytest.raises(RuntimeError, match="WASM lock boundary failure"):
            run()
        assert not phases
    else:
        assert not run()
        assert phases[0][0][0] == "build_lock"
        assert phases[0][1]["mode"] == "rejected"
        assert "WASM build lock acquisition failed" in capsys.readouterr().err


def test_prewarm_receipt_inner_runtime_error_is_not_a_lock_failure(
    tmp_path, monkeypatch
):
    from molt.cli import backend_build

    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)

    def broken_receipt(*args, **kwargs):
        raise RuntimeError("receipt work programming error")

    monkeypatch.setattr(backend_build, "_verified_backend_receipts", broken_receipt)
    with pytest.raises(RuntimeError, match="receipt work programming error"):
        _dispatch(["internal-backend-build", "--target", "native", "--json"])


def test_cold_backend_success_identity_is_captured_under_admission_lock(
    tmp_path, monkeypatch
):
    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    original_lock = backend_binary._backend_admission_lock
    original_success = backend_binary._backend_ensure_success
    held = False

    @contextmanager
    def tracked_lock(*args, **kwargs):
        nonlocal held
        with original_lock(*args, **kwargs):
            held = True
            try:
                yield
            finally:
                held = False

    def checked_success(*args, **kwargs):
        assert held, (
            "admitted bytes must be projected before another lane overwrites them"
        )
        return original_success(*args, **kwargs)

    monkeypatch.setattr(backend_binary, "_backend_admission_lock", tracked_lock)
    monkeypatch.setattr(backend_binary, "_backend_ensure_success", checked_success)
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 0
