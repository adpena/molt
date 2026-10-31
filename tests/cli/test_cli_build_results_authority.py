from __future__ import annotations

from molt.compilation_target import admit_target

import inspect
import json
import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest

import molt.cli as cli
from molt.capability_manifest import CapabilityManifest
from molt.cli import build_results, link_fingerprints, progress

_BUILD_RESULTS_NAMES = (
    "_attach_build_metadata",
    "_attach_process_output",
    "_build_cache_info",
    "_build_common_build_json_data",
    "_build_native_link_error_data",
    "_build_native_link_success_data",
    "_emit_build_result_json",
    "_emit_native_link_result",
    "_emit_non_native_build_result",
    "_post_link_strip",
)


@pytest.mark.parametrize("mutation", [None, "rewrite", "delete"])
def test_native_finalization_fences_source_after_signing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str | None
) -> None:
    from molt.cli import atomic_io
    from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease
    from tests.native_artifact_fixtures import signed_macho_image

    source = tmp_path / "app.py"
    source.write_text("value = 1\n", encoding="utf-8")
    catalog = _ModuleSourceCatalog({"app": _ModuleSourceLease.path_backed(source)})
    # A Mach-O image another tool wrote is re-signed as the published name,
    # the last step before publication.
    candidate = tmp_path / "candidate"
    program = bytes(signed_macho_image(candidate.name))
    candidate.write_bytes(program)
    output = tmp_path / "app"
    output.write_bytes(b"previous program")

    def sign(stage: Path, destination: Path) -> None:
        assert stage != output and destination == output
        assert output.read_bytes() == b"previous program"
        if mutation == "rewrite":
            source.write_text("value = 2\n", encoding="utf-8")
        elif mutation == "delete":
            source.unlink()

    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", sign)
    monkeypatch.delenv("MOLT_BUILD_SMOKE_EXEC", raising=False)
    monkeypatch.delenv("MOLT_SKIP_BINARY_VALIDITY_CHECK", raising=False)
    error = build_results._finalize_native_link_candidate(
        candidate=candidate,
        output_binary=output,
        target_triple="aarch64-apple-darwin",
        strip=False,
        validate_inputs=catalog.validate,
    )
    if mutation is None:
        assert error is None
        assert output.read_bytes() == program
    else:
        assert error is not None and "source" in error
        assert output.read_bytes() == b"previous program"
    assert not candidate.exists()


@pytest.mark.parametrize("kind", ["output_alias", "hardlink", "symlink"])
def test_native_finalizer_does_not_reclaim_unadmitted_inputs(
    tmp_path: Path, kind: str
) -> None:
    import os

    output = tmp_path / "program"
    output.write_bytes(b"previous public binary")
    unrelated = tmp_path / "unrelated"
    unrelated.write_bytes(b"retained input")
    candidate = tmp_path / "candidate"
    if kind == "output_alias":
        candidate = output
    elif kind == "hardlink":
        os.link(unrelated, candidate)
    else:
        try:
            candidate.symlink_to(unrelated)
        except OSError as exc:
            if os.name == "nt" and getattr(exc, "winerror", None) == 1314:
                pytest.skip(
                    "Windows symlink creation requires the owning host privilege"
                )
            raise
    error = build_results._finalize_native_link_candidate(
        candidate=candidate,
        output_binary=output,
        target_triple="x86_64-unknown-linux-gnu",
        strip=False,
    )
    assert error is not None and "requires a private candidate" in error
    assert candidate.exists()
    assert output.read_bytes() == b"previous public binary"
    assert unrelated.read_bytes() == b"retained input"


_BUILD_RESULTS_DEFINITIONS = (
    "def _attach_build_metadata(",
    "def _attach_process_output(",
    "def _build_cache_info(",
    "def _build_common_build_json_data(",
    "def _build_native_link_error_data(",
    "def _build_native_link_success_data(",
    "def _emit_build_result_json(",
    "def _emit_native_link_result(",
    "def _emit_non_native_build_result(",
    "def _post_link_strip(",
)


def test_cli_build_results_authority_is_single_home() -> None:
    assert hasattr(link_fingerprints, "publish_link_outputs")
    assert not hasattr(link_fingerprints, "_write_link_fingerprint_if_needed")
    assert not hasattr(build_results, "_write_link_fingerprint_if_needed")
    assert not hasattr(cli, "_write_link_fingerprint_if_needed")
    for name in _BUILD_RESULTS_NAMES:
        assert hasattr(build_results, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for marker in _BUILD_RESULTS_DEFINITIONS:
        assert marker not in cli_source


def _emit_link_result(
    tmp_path: Path,
    process: subprocess.CompletedProcess[str],
    *,
    json_output: bool,
    finalize_inputs: Callable[[], None] | None = None,
) -> int:
    return build_results._emit_native_link_result(
        link_process=process,
        link_skipped=False,
        link_fingerprint=None,
        link_fingerprint_path=tmp_path / "link.json",
        cache=False,
        cache_hit=False,
        cache_key=None,
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        cache_hit_tier=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_config_digest=None,
        target=admit_target("native"),
        target_triple=None,
        source_path=tmp_path / "app.py",
        output_binary=tmp_path / "app.exe",
        deterministic=False,
        trusted=False,
        resolved_capability_policy=CapabilityManifest().resolve(),
        capabilities_source=None,
        sysroot_path=None,
        emit_mode="binary",
        profile="dev",
        native_arch_perf_enabled=False,
        output_obj=tmp_path / "app.lib",
        stub_path=tmp_path / "main_stub.c",
        runtime_lib=tmp_path / "runtime.lib",
        external_native_artifacts=(),
        diagnostics_enabled=False,
        build_diagnostics_payload=lambda: (None, None),
        pgo_profile_payload=None,
        runtime_feedback_payload=None,
        emit_ir_path=None,
        stdlib_obj_path=None,
        warnings=[],
        json_output=json_output,
        resolved_diagnostics_verbosity="brief",
        finalize_inputs=finalize_inputs,
    )


@pytest.mark.parametrize(
    ("stdout", "stderr", "expected"),
    [
        (
            "driver detail",
            "lld-link: unresolved symbol\n",
            "driver detail\nlld-link: unresolved symbol\nLinking failed\n",
        ),
        ("driver-only diagnostic\n", "", "driver-only diagnostic\nLinking failed\n"),
    ],
)
def test_native_link_failure_text_preserves_captured_streams(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    stdout: str,
    stderr: str,
    expected: str,
) -> None:
    process = subprocess.CompletedProcess(["clang"], 1, stdout, stderr)
    assert _emit_link_result(tmp_path, process, json_output=False) == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == expected


def test_native_link_failure_json_keeps_structured_streams(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    process = subprocess.CompletedProcess(
        ["clang"], 1, "driver detail", "lld-link: unresolved symbol\n"
    )
    assert _emit_link_result(tmp_path, process, json_output=True) == 1
    captured = capsys.readouterr()
    assert captured.err == ""
    payload = json.loads(captured.out)
    assert payload["errors"] == ["Linking failed"]
    assert payload["data"]["stdout"] == process.stdout
    assert payload["data"]["stderr"] == process.stderr


def test_native_link_failure_keeps_diagnostics_in_quiet_mode(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    process = subprocess.CompletedProcess(
        ["clang"], 1, "", "lld-link: missing import\n"
    )
    with progress.BuildProgress(quiet=True):
        assert _emit_link_result(tmp_path, process, json_output=False) == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == "lld-link: missing import\nLinking failed\n"


def test_native_link_success_text_preserves_linker_warnings(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(build_results, "_assert_native_binary_valid", lambda *_: None)
    monkeypatch.setattr(
        build_results,
        "_native_artifact_binary_image_analysis_payload",
        lambda **_: {},
    )
    process = subprocess.CompletedProcess(
        ["clang"], 0, "driver warning\n", "lld-link: warning: weak symbol"
    )
    assert _emit_link_result(tmp_path, process, json_output=False) == 0
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == (
        "driver warning\nlld-link: warning: weak symbol\n"
        f"Successfully built {tmp_path / 'app.exe'}\n"
    )


@pytest.mark.parametrize("target", ["native", "wasm"])
@pytest.mark.parametrize("json_output", [False, True])
def test_input_custody_failure_prevents_success_result(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
    target: str,
    json_output: bool,
) -> None:
    closed = []

    def finish_inputs() -> None:
        closed.append(True)
        raise ValueError("owned producer did not exit")

    monkeypatch.setattr(build_results, "_assert_native_binary_valid", lambda *_: None)
    if target == "native":
        result = _emit_link_result(
            tmp_path,
            subprocess.CompletedProcess(["clang"], 0, "", ""),
            json_output=json_output,
            finalize_inputs=finish_inputs,
        )
    else:
        result = build_results._emit_non_native_build_result(
            output=tmp_path / "app.wasm",
            consumer_output=None,
            bundle_root=None,
            cache=False,
            cache_hit=False,
            cache_key=None,
            function_cache_key=None,
            cache_path=None,
            function_cache_path=None,
            cache_hit_tier=None,
            backend_daemon_cached=None,
            backend_daemon_cache_tier=None,
            backend_daemon_config_digest=None,
            target=admit_target("wasm"),
            target_triple="wasm32-wasip1",
            source_path=tmp_path / "app.py",
            deterministic=False,
            trusted=False,
            resolved_capability_policy=CapabilityManifest().resolve(),
            capabilities_source=None,
            sysroot_path=None,
            emit_mode="binary",
            profile="dev",
            native_arch_perf_enabled=False,
            diagnostics_enabled=False,
            build_diagnostics_payload=lambda: (None, None),
            pgo_profile_payload=None,
            runtime_feedback_payload=None,
            emit_ir_path=None,
            warnings=[],
            json_output=json_output,
            resolved_diagnostics_verbosity="brief",
            success_messages=["Successfully built app.wasm"],
            finalize_inputs=finish_inputs,
        )
    assert result == 1
    assert closed == [True]
    captured = capsys.readouterr()
    if json_output:
        payload = json.loads(captured.out)
        assert payload["status"] == "error"
        assert payload["errors"] == [
            "Build input custody failed to close: owned producer did not exit"
        ]
        assert "messages" not in payload["data"]
    else:
        assert captured.out == ""
        assert "owned producer did not exit" in captured.err
        assert "Successfully built" not in captured.err


def _native_record_fixture(tmp_path: Path):
    from dataclasses import replace
    from molt.build_record import NativeBuildResult, RecordedArtifact, NativeBuildFacts
    from molt.capability_policy import CapabilityPolicy, resolve_capability_policy
    from molt.backend_protocol import (
        BackendEmissionOrigin,
        BackendEmissionProducer,
        BackendAddressedProgramInput,
        BackendCodegenOrigin,
    )

    paths = {
        name: tmp_path / name
        for name in ("binary", "archive", "stub", "runtime", "compiler")
    }
    for name, path in paths.items():
        path.write_text(f"owned {name}", encoding="utf-8")
    result = NativeBuildResult(
        NativeBuildFacts(
            None,
            str(tmp_path / "app.py"),
            False,
            False,
            (),
            (),
            resolve_capability_policy(CapabilityPolicy()).digest(),
            None,
            "dev",
            False,
            "generic",
            "default",
        ),
        *(RecordedArtifact.capture(path) for path in paths.values()),
        (),
        None,
    )
    origin = BackendEmissionOrigin(
        BackendEmissionProducer("daemon", 12345, None),
        BackendAddressedProgramInput("b" * 64, 17, ""),
        result.archive.sha256,
        result.archive.token.size,
        BackendCodegenOrigin(12345, ()),
    )
    result = replace(result, emission_origin=origin)
    data = result.facts.payload()
    data.update(
        {
            "target": "native",
            "emit": "bin",
            "sysroot": None,
            "output": str(result.binary.path),
            "consumer_output": str(result.binary.path),
            "artifacts": {
                name: str(paths[name])
                for name in ("binary", "archive", "stub", "runtime")
            },
            "observed_toolchain": build_results._observed_build_toolchain(
                backend_bin=paths["compiler"],
                runtime_lib=paths["runtime"],
                output=paths["binary"],
                emission_origin=origin,
            ),
        }
    )
    return result, data


def test_native_reuse_projects_current_facts_through_actual_consumers(
    tmp_path: Path, capsys
) -> None:
    from molt.cli.wrapper_build import _parse_wrapper_build_contract_payload
    from tools.dx_build_timer import _molt_build_artifact, MoltMeasurement

    result, _data = _native_record_fixture(tmp_path)
    assert build_results._emit_reused_native_build_result(result) == 0
    stdout = capsys.readouterr().out
    payload = json.loads(stdout)
    contract, error = _parse_wrapper_build_contract_payload(
        payload, json_output=True, command="run"
    )
    assert error is None and contract is not None
    assert contract.consumer_output == result.binary.path
    assert set(contract.artifacts) == {"binary", "archive", "stub", "runtime"}
    measured = _molt_build_artifact(
        stdout, result.binary.path, MoltMeasurement.ATTRIBUTION
    )
    assert measured["identity"]["sha256"] == result.binary.sha256
    data = payload["data"]
    assert data["cache"] == {"enabled": True, "hit": True, "hit_tier": "build-record"}
    assert data["link"] == {"skipped": True}
    assert payload["warnings"] == []
    assert not {"stdout", "stderr", "compile_diagnostics"} & data.keys()
    assert data["observed_toolchain"]["compiled_with_verified"] is False
    assert result.emission_origin is not None
    assert (
        data["observed_toolchain"]["emission_origin"]
        == result.emission_origin.payload()
    )
    assert (
        data["observed_toolchain"]["emission_origin"]["producer"]["process_id"] == 12345
    )


@pytest.mark.parametrize("mutation", [None, "archive", "stub", "runtime"])
def test_native_result_reporting_keeps_pre_link_expected_generation(
    tmp_path: Path, monkeypatch, mutation: str | None
) -> None:
    import time
    from molt import build_record, input_report

    result, data = _native_record_fixture(tmp_path)
    if mutation is not None:
        getattr(result, mutation).path.write_text(
            "replacement after link", encoding="utf-8"
        )
    ledger = build_record.InputLedger(time.time_ns())
    monkeypatch.setattr(input_report, "_SINK", ledger)
    monkeypatch.setattr(input_report, "_CAPTURE_RESULT", True)
    build_results._record_native_build_result(
        data=data,
        result_inputs=(result.archive, result.stub, result.runtime),
        warnings=(),
        diagnostics_error=None,
    )
    if mutation is None:
        assert ledger.result == result and not ledger.refusals
    else:
        assert ledger.result is None
        assert any("changed" in reason for reason in ledger.refusals)


@pytest.mark.parametrize(
    "field",
    [
        "warning",
        "stdout",
        "stderr",
        "compile_diagnostics",
        "external_native_artifacts",
        "unclassified_new_result",
    ],
)
def test_invocation_specific_native_results_are_not_recorded(
    tmp_path: Path, monkeypatch, field: str
) -> None:
    import time
    from molt import build_record, input_report

    result, data = _native_record_fixture(tmp_path)
    warnings = ["original warning"] if field == "warning" else []
    if field != "warning":
        data[field] = "original invocation detail"
    ledger = build_record.InputLedger(time.time_ns())
    monkeypatch.setattr(input_report, "_SINK", ledger)
    monkeypatch.setattr(input_report, "_CAPTURE_RESULT", True)
    build_results._record_native_build_result(
        data=data,
        result_inputs=(result.archive, result.stub, result.runtime),
        warnings=warnings,
        diagnostics_error=None,
    )
    assert ledger.result is None and ledger.refusals
