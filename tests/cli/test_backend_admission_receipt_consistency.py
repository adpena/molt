from __future__ import annotations

import os

from tests.compiler_identity_helper import (
    compiler_environment_inputs,
    compiler_cargo_stdout,
)
from molt.process_environment import ProcessEnvironment
from tests.runtime_build_identity_helper import runtime_cargo_environment_inputs

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
    from molt.cli import backend_build

    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    original = backend_binary._build_lock
    original_receipts = backend_build._verified_backend_receipts
    receipt_reads = 0

    def interleaved_once(*args, **kwargs):
        # The receipt read runs without the lock first; an interleaving lane
        # makes it fail, so the second read takes the lock.
        nonlocal receipt_reads
        receipt_reads += 1
        if receipt_reads == 1:
            return None, "another lane interleaved"
        return original_receipts(*args, **kwargs)

    monkeypatch.setattr(backend_build, "_verified_backend_receipts", interleaved_once)
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

    monkeypatch.setattr(backend_binary, "_run_resolved_cargo_plan", unavailable)
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 2
    captured = capsys.readouterr()
    payload = json.loads(captured.out)
    assert payload["data"]["failure"]["phase"] == "backend_cargo_build"
    assert "cargo executable missing" in captured.err
    assert payload["data"]["stage_timings_ms"]["backend_binary_cargo_build"] >= 0


@pytest.mark.parametrize("target", ["native", "wasm", "luau", "rust"])
@pytest.mark.parametrize("failure_stage", ["initial", "wrapper", "feature-probe"])
def test_backend_admission_preserves_lockfile_on_its_single_cargo_attempt(
    target, failure_stage, tmp_path, monkeypatch, capsys
):
    import subprocess

    from molt.cli import cargo_execution

    root = _isolated_molt_root(tmp_path, monkeypatch)
    lockfile = root / "Cargo.lock"
    original_lock = b"version = 4\n# existing dependency resolution\n"
    lockfile.write_bytes(original_lock)
    _fake_backend_toolchain(monkeypatch)
    monkeypatch.setattr(
        backend_binary,
        "_run_resolved_cargo_plan",
        cargo_execution._run_resolved_cargo_plan,
    )
    if failure_stage == "wrapper":
        monkeypatch.setenv("RUSTC_WRAPPER", "sccache")
    attempts = []
    probes = []
    stale_lock_error = "the lock file needs to be updated but --locked was passed"

    def external(cmd, **kwargs):
        # Keep the real resolved-plan executor and its capacity boundary. Model
        # the lockfile mutation an accidentally unlocked Cargo call would allow.
        attempts.append((list(cmd), kwargs["env"].get("RUSTC_WRAPPER")))
        if failure_stage == "wrapper":
            return subprocess.CompletedProcess(
                cmd, 2, "", "sccache: error: cache server unavailable"
            )
        if failure_stage == "initial":
            if "--locked" in cmd:
                return subprocess.CompletedProcess(cmd, 101, "", stale_lock_error)
            lockfile.write_bytes(b"version = 4\n# unexpectedly updated resolution\n")
        output = (
            Path(kwargs["env"]["CARGO_TARGET_DIR"])
            / "release"
            / ("molt-backend.exe" if os.name == "nt" else "molt-backend")
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(b"backend executable")
        output.chmod(0o755)
        return subprocess.CompletedProcess(
            cmd, 0, compiler_cargo_stdout(root, output, kwargs["env"]), ""
        )

    def probe(cmd, **kwargs):
        probes.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 1, b"", b"feature mismatch")

    monkeypatch.setattr(cargo_execution, "_run_completed_command", external)
    monkeypatch.setattr(backend_binary, "_run_subprocess_captured_to_tempfiles", probe)
    result = _dispatch(["internal-backend-build", "--target", target, "--json"])
    captured = capsys.readouterr()
    payload = json.loads(captured.out)
    assert lockfile.read_bytes() == original_lock
    assert result == 2
    assert payload["status"] == "error"
    assert payload["data"]["failure"]["phase"] == (
        "backend_feature_probe"
        if failure_stage == "feature-probe"
        else "backend_cargo_build"
    )
    expected_code, expected_message = {
        "initial": (101, stale_lock_error),
        "wrapper": (2, "sccache: error: cache server unavailable"),
        "feature-probe": (1, "feature mismatch"),
    }[failure_stage]
    assert payload["data"]["failure"]["returncode"] == expected_code
    assert expected_message in captured.err
    assert "receipts" not in payload["data"]
    assert len(attempts) == 1
    assert attempts[0][0].count("--locked") == 1
    assert len(probes) == (1 if failure_stage == "feature-probe" else 0)
    if failure_stage == "wrapper":
        assert attempts[0][1] == "sccache"


def test_the_backend_lock_guards_the_cargo_output_of_its_coordinates_target(
    tmp_path, monkeypatch
):
    """Variants of one target share a lock; another target's build never waits.

    The lock guards Cargo's one output beside a coordinate, so it lives in that
    coordinate's target build control, not in whatever target the project
    selects by default.
    """
    monkeypatch.delenv("MOLT_BUILD_STATE_DIR", raising=False)
    monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", "0.2")
    native = tmp_path / "a" / "release" / "molt-backend.native_backend"
    wasm = native.with_name("molt-backend.wasm_backend")
    other_target = tmp_path / "b" / "release" / native.name
    with backend_binary._backend_admission_lock(tmp_path, native, "release"):
        with backend_binary._backend_admission_lock(tmp_path, other_target, "release"):
            pass
        with pytest.raises(
            backend_binary._BackendAdmissionLockError, match="Timed out"
        ):
            with backend_binary._backend_admission_lock(tmp_path, wasm, "release"):
                pytest.fail("two variants of one target held its lock at once")


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_an_incomplete_identity_never_selects_or_renames_a_stored_compiler(
    target, tmp_path, monkeypatch, capsys
):
    """A store key needs the whole product identity; a partial one fails closed.

    The receipt-filling tolerance for an unknown rustc belonged to per-checkout
    selections. Admission always knows rustc from executable custody, so a
    fingerprint without it is a defect: it must neither reuse the stored
    compiler under another name nor rebuild.
    """
    _isolated_molt_root(tmp_path, monkeypatch)
    cargo, probes = _fake_backend_toolchain(monkeypatch)
    argv = ["internal-backend-build", "--target", target, "--json"]
    assert _dispatch(argv) == 0
    before = json.loads(capsys.readouterr().out)["data"]
    receipt = Path(before["receipts"]["source_content"]["path"])
    stored = receipt.read_bytes()
    original = backend_binary._backend_fingerprint

    def no_rustc(*args, **kwargs):
        fingerprint = original(*args, **kwargs)
        fingerprint["rustc"] = None
        return fingerprint

    monkeypatch.setattr(backend_binary, "_backend_fingerprint", no_rustc)
    assert _dispatch(argv) == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == "backend_source_identity"
    assert "identity is incomplete" in payload["errors"][0]
    assert len(cargo) == len(probes) == 1
    assert receipt.read_bytes() == stored


def test_a_removed_stored_compiler_is_corruption_until_its_entry_is_removed(
    tmp_path, monkeypatch, capsys
):
    """Removing part of a published entry fails loud; evicting it rebuilds.

    The message names the entry and the tool that evicts it. Once the tool
    has removed it, the next build runs Cargo, publishes and probes once for
    the new generation.
    """
    _isolated_molt_root(tmp_path, monkeypatch)
    cargo, probes = _fake_backend_toolchain(monkeypatch)
    argv = ["internal-backend-build", "--target", "wasm", "--json"]
    assert _dispatch(argv) == 0
    generation = Path(probes[0][0])
    generation.unlink()
    capsys.readouterr()
    assert _dispatch(argv) == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == "product_store"
    [message] = payload["errors"]
    assert "is corrupt" in message
    # <entry>/<variant>/.molt-backend-generations/<tool>/<sha256>/<tool>
    entry = generation.parents[4]
    assert str(entry) in message
    assert (len(cargo), len(probes)) == (1, 1)
    assert f"`molt cache evict host-tool {entry.name}`" in message

    assert _dispatch(["cache", "evict", "host-tool", entry.name, "--json"]) == 0
    assert json.loads(capsys.readouterr().out)["data"]["evicted"]
    assert not entry.exists()
    assert _dispatch(argv) == 0
    assert (len(cargo), len(probes)) == (2, 2)
    # The rebuild published and probed a whole new entry.
    assert Path(probes[1][0]).is_file() and Path(probes[1][0]).is_relative_to(entry)


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
        cargo_environment=compiler_environment_inputs(),
    )
    before = receipt.read_bytes()
    source = runtime_fingerprints._admitted_runtime_fingerprint(
        source, runtime_fingerprints._read_runtime_fingerprint(receipt)
    )

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
        "_run_resolved_cargo_plan",
        cargo_execution._run_resolved_cargo_plan,
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
def test_feature_probe_failure_does_not_retry_a_different_build_environment(
    failure_kind, tmp_path, monkeypatch, capsys
):
    import subprocess
    from molt.cli import cargo_execution

    root = _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    monkeypatch.setattr(
        backend_binary,
        "_run_resolved_cargo_plan",
        cargo_execution._run_resolved_cargo_plan,
    )
    external_calls = []

    def external(cmd, **kwargs):
        external_calls.append(cmd)
        if len(external_calls) > 1:
            raise FileNotFoundError("unexpected retry cargo vanished")
        output = (
            Path(kwargs["env"]["CARGO_TARGET_DIR"])
            / "release"
            / ("molt-backend.exe" if os.name == "nt" else "molt-backend")
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(b"backend executable")
        output.chmod(0o755)
        return subprocess.CompletedProcess(
            cmd, 0, compiler_cargo_stdout(root, output, kwargs["env"]), ""
        )

    def mismatch(cmd, **kwargs):
        if failure_kind == "capacity":
            monkeypatch.setenv("MOLT_DISK_GUARD_HIGH_WATER_GB", "abc")
        return subprocess.CompletedProcess(cmd, 1, b"", b"feature mismatch")

    monkeypatch.setattr(cargo_execution, "_run_completed_command", external)
    monkeypatch.setattr(
        backend_binary, "_run_subprocess_captured_to_tempfiles", mismatch
    )
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == "backend_feature_probe"
    assert payload["data"]["stage_timings_ms"]["backend_binary_cargo_build"] >= 0
    assert payload["data"]["stage_timings_ms"]["backend_binary_probe"] >= 0
    assert "feature mismatch" in payload["errors"][0]
    assert len(external_calls) == 1


def test_generation_copy_rejects_bytes_changed_after_admission(
    tmp_path, monkeypatch, capsys
):
    from molt.cli import backend_generation

    _isolated_molt_root(tmp_path, monkeypatch)
    cargo, probes = _fake_backend_toolchain(monkeypatch)
    argv = ["internal-backend-build", "--target", "wasm", "--json"]
    assert _dispatch(argv) == 0
    initial = json.loads(capsys.readouterr().out)["data"]
    generation = Path(initial["compiler"]["path"])
    selection_receipt = Path(initial["receipts"]["source_content"]["path"])
    generation.unlink()
    selection_receipt.unlink()
    original = backend_generation._staged_copy_file

    def changed_copy(source, destination, **kwargs):
        source.write_bytes(b"not the compiler that was admitted")
        return original(source, destination, **kwargs)

    monkeypatch.setattr(backend_generation, "_staged_copy_file", changed_copy)
    assert _dispatch(argv) == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == "backend_artifact"
    assert not selection_receipt.exists()
    assert not generation.exists()
    assert len(cargo) == 2


def test_build_lock_real_timeout_does_not_mask_inner_failure(tmp_path, monkeypatch):
    from molt.file_locks import _acquire_file_lock, _release_file_lock

    monkeypatch.setattr(
        build_locks, "_build_state_root", lambda root, cargo_target=None: tmp_path
    )
    lock_path = tmp_path / "build_locks" / "held.lock"
    handle = _acquire_file_lock(
        lock_path, timeout_s=1, timeout_message="initial lock unavailable"
    )
    monkeypatch.setenv("MOLT_BUILD_LOCK_TIMEOUT", "0.02")
    try:
        with pytest.raises(build_locks.BuildLockAcquisitionError, match="Timed out"):
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

    monkeypatch.setattr(build_locks, "_try_acquire_file_lock", unavailable)
    ready, error = backend_compile._start_backend_daemon_under_lock(
        tmp_path / "backend",
        tmp_path / "daemon.sock",
        cargo_profile="release",
        project_root=tmp_path,
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


@pytest.mark.parametrize("inside_work", [False, True])
def test_wasm_member_lock_boundary_distinguishes_acquisition_from_work(
    inside_work, tmp_path, monkeypatch, capsys
):
    from molt.cli import runtime_wasm_build, build_locks
    from molt.cli.runtime_wasm_build_spec import _RuntimeWasmBuildSpec
    from molt.cli.runtime_artifact_selection import RUNTIME_CDYLIB_ARTIFACTS

    spec = _RuntimeWasmBuildSpec(
        publication_name="runtime.wasm",
        requested_cargo_profile="dev-fast",
        cargo_profile="dev-fast",
        profile_dir="dev-fast",
        incremental_enabled=False,
        env=ProcessEnvironment({}),
        artifact_selection=RUNTIME_CDYLIB_ARTIFACTS,
        runtime_exports=(),
        link_flags=(),
        cargo_rustflags=(),
        fingerprint_rustflags=(),
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
        monkeypatch.setattr(build_locks, "_try_acquire_file_lock", broken)

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
            cargo_environment=runtime_cargo_environment_inputs(),
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


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_published_lane_is_admitted_while_another_lane_holds_the_lock(
    target, tmp_path, monkeypatch, capsys
):
    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    assert _dispatch(["internal-backend-build", "--target", target, "--json"]) == 0
    capsys.readouterr()

    @contextmanager
    def held(*args, **kwargs):
        raise RuntimeError("held by another lane's cold build")
        yield

    # The lock guards Cargo's shared output; reusing this lane's own published,
    # receipt-bound executable must not wait for it.
    monkeypatch.setattr(backend_binary, "_build_lock", held)
    assert _dispatch(["internal-backend-build", "--target", target, "--json"]) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "ok"
    assert (
        payload["data"]["stage_timings_ms"]["backend_binary_published_admission"] >= 0
    )

    # Stored bytes that no longer match their receipt are corruption: the
    # build fails loud by reads alone, without waiting for the lock.
    binary = Path(payload["data"]["compiler"]["path"])
    binary.write_bytes(binary.read_bytes() + b"changed")
    assert _dispatch(["internal-backend-build", "--target", target, "--json"]) == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["failure"]["phase"] == "product_store"
    assert "its executable differs from its receipt" in payload["errors"][0]
