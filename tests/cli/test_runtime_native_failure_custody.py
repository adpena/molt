from __future__ import annotations

import contextlib
import json
import subprocess
from pathlib import Path

import pytest

from molt.cli import runtime_native_build as runtime
from molt.cli.cargo_execution import CargoExecutionResult
from molt.cli.models import _RuntimeArtifactState
from tests.cli.native_link_test_support import write_test_static_archive
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    native_runtime_staticlib_identity,
    runtime_cargo_plan,
)


@pytest.fixture
def plan(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
):
    identity = native_runtime_staticlib_identity(cargo_profile="dev-fast")
    archive = tmp_path / "molt_runtime.lib"
    write_test_static_archive(archive)
    monkeypatch.setattr(runtime, "_build_state_root", lambda _root: tmp_path / "state")
    monkeypatch.setattr(runtime, "_build_slot", lambda: contextlib.nullcontext())
    monkeypatch.setattr(
        runtime, "_runtime_cargo_scratch_lib_path", lambda *_args: archive
    )
    monkeypatch.setattr(
        runtime._NativeRuntimeBuildPlan,
        "identity_is_current",
        lambda *_args, **_kw: True,
    )
    monkeypatch.setattr(
        runtime._NativeRuntimeBuildPlan,
        "accept",
        lambda *_args, **_kw: pytest.fail("failed artifact must never be admitted"),
    )
    return runtime._NativeRuntimeBuildPlan(
        runtime_lib=archive,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        cargo_timeout=1.0,
        stage_timings_ms=None,
        runtime_state=_RuntimeArtifactState(),
        cargo_plan=runtime_cargo_plan(
            tmp_path,
            fixture_root=runtime_fixture_root,
            env={},
            cargo_command=("cargo", "rustc"),
        ),
        fingerprint_features=("stdlib_micro",),
        build_identity=identity,
        candidates=(),
    )


@pytest.mark.parametrize(
    "error_type",
    [
        OSError,
        ValueError,
        runtime.NativeLinkCustodyError,
        runtime.NativeLinkDependencyManifestError,
    ],
)
def test_native_generation_failure_rejects_artifact(
    plan, monkeypatch: pytest.MonkeyPatch, error_type
) -> None:
    def fail(*_args, **_kwargs):
        raise error_type("generation custody failure")

    monkeypatch.setattr(runtime, "publish_native_runtime_generation", fail)
    result = CargoExecutionResult(
        subprocess.CompletedProcess(plan.cmd, 0, "cargo output", "cargo warning"),
        attempts=(),
        retry_reason=None,
    )
    assert not runtime._publish_native_runtime_build(plan, result)
    failure = plan.runtime_state.native_runtime_build_failure
    assert failure is not None
    assert failure.stage == "generation-publication"
    assert "generation custody failure" in failure.summary
    assert failure.evidence_path is not None
    payload = json.loads(failure.evidence_path.read_text(encoding="utf-8"))
    assert payload["command"] == plan.cmd
    assert payload["cargo_stdout"] == "cargo output"
    assert payload["cargo_stderr"] == "cargo warning"


@pytest.mark.parametrize("as_bytes", [False, True])
def test_native_cargo_timeout_retains_partial_output(
    plan, monkeypatch: pytest.MonkeyPatch, as_bytes: bool
) -> None:
    stdout = "partial Cargo artifact message"
    stderr = "error: partial compiler diagnostic"

    def timeout(*_args, **_kwargs):
        raise subprocess.TimeoutExpired(
            plan.cmd,
            1.0,
            output=stdout.encode() if as_bytes else stdout,
            stderr=stderr.encode() if as_bytes else stderr,
        )

    monkeypatch.setattr(runtime, "_run_resolved_cargo_plan", timeout)
    assert not runtime._build_native_runtime_under_lock(plan)
    failure = plan.runtime_state.native_runtime_build_failure
    assert failure is not None and failure.timed_out
    assert "partial compiler diagnostic" in failure.summary
    assert failure.evidence_path is not None
    payload = json.loads(failure.evidence_path.read_text(encoding="utf-8"))
    assert payload["cargo_stdout"] == stdout
    assert payload["cargo_stderr"] == stderr
    assert payload["timed_out"] is True
    assert payload["command"] == plan.cmd


@pytest.mark.parametrize("attach_state", [False, True])
def test_native_failure_evidence_publication_failure_is_observable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys, attach_state: bool
) -> None:
    def fail(*_args, **_kwargs):
        raise OSError("evidence volume unavailable")

    state = _RuntimeArtifactState() if attach_state else None
    monkeypatch.setattr(runtime, "_build_state_root", lambda _root: tmp_path)
    monkeypatch.setattr(runtime, "_atomic_write_json", fail)
    assert not runtime._record_native_runtime_failure(
        state,
        project_root=tmp_path,
        stage="cargo",
        summary="Original compile failure",
    )
    if state is not None:
        failure = state.native_runtime_build_failure
        assert failure is not None and failure.evidence_path is None
        signal = failure.summary
    else:
        signal = capsys.readouterr().err
    assert "Original compile failure" in signal
    assert "evidence volume unavailable" in signal


def test_disabled_native_rebuild_never_runs_cargo_for_provenance(
    plan, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    monkeypatch.setattr(
        runtime,
        "_run_resolved_cargo_plan",
        lambda *_a, **_k: pytest.fail("rebuild-disabled policy invoked Cargo"),
    )
    assert not runtime._build_native_runtime_under_lock(plan)
    failure = plan.runtime_state.native_runtime_build_failure
    assert failure is not None and failure.stage == "rebuild-policy"
    assert "MOLT_SKIP_RUNTIME_REBUILD=1" in failure.summary
    assert failure.evidence_path is not None
    payload = json.loads(failure.evidence_path.read_text(encoding="utf-8"))
    assert payload["command"] == plan.cmd


