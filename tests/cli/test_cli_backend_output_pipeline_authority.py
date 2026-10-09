from __future__ import annotations

import inspect
import json
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.capability_manifest import CapabilityManifest
from molt.cli import (
    backend_pipeline,
    build_diagnostics,
    build_results,
    frontend_pipeline,
    output,
)
from molt.cli.models import _BackendCacheSetup, _BuildDiagnosticsContext
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.runtime_build_python import BuildPythonAdmission

import molt.cli as cli
from molt.cli import backend_output_pipeline as cli_backend_output_pipeline
from molt.cli import build_pipeline as cli_build_pipeline


_BACKEND_OUTPUT_PIPELINE_NAMES = {
    "_emit_backend_pipeline_outputs",
}


def test_backend_output_pipeline_authority_lives_in_backend_output_module() -> None:
    for name in _BACKEND_OUTPUT_PIPELINE_NAMES:
        owner = getattr(cli_backend_output_pipeline, name)
        assert inspect.getmodule(owner) is cli_backend_output_pipeline
        assert not hasattr(cli_build_pipeline, name)
        assert not hasattr(cli, name)


@pytest.fixture
def terminal_build(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, request: pytest.FixtureRequest
):
    clock = SimpleNamespace(now=10.0)
    monkeypatch.setattr(build_diagnostics.time, "perf_counter", lambda: clock.now)
    monkeypatch.setattr(
        build_diagnostics, "_runtime_wasm_cache_diagnostics_snapshot", lambda: None
    )
    monkeypatch.setattr(
        build_diagnostics, "_runtime_wasm_build_timings_snapshot", lambda: None
    )
    monkeypatch.setattr(
        build_diagnostics, "_build_midend_diagnostics_payload", lambda **_: None
    )
    phases = {"resolve_entry": 0.0, "runtime_setup": 8.0}
    parallel: dict[str, object] = {}
    diagnostics_file = tmp_path / "build-diagnostics.json"
    context = _BuildDiagnosticsContext(
        diagnostics_enabled=True,
        diagnostics_start=0.0,
        phase_starts=phases,
        image_scope=None,
        binary_image_closure=None,
        binary_image_analysis=None,
        module_graph={},
        module_graph_operation_counts={},
        module_reasons={},
        frontend_module_timings=[],
        allocation_diagnostics_enabled=False,
        frontend_parallel_details=parallel,
        profile="dev",
        midend_policy_outcomes_by_function={},
        midend_pass_stats_by_function={},
        backend_daemon_health=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_config_digest=None,
        diagnostics_path_spec=str(diagnostics_file),
        artifacts_root=tmp_path,
    )
    state = SimpleNamespace(
        clock=clock,
        phases=phases,
        context=context,
        diagnostics_file=diagnostics_file,
        snapshots=[],
        analyses=[],
        identity_calls=[],
        identity_cost=0.0,
        events=[],
        failure=None,
        link_hit=False,
    )

    def snapshot():
        state.events.append("snapshot")
        state.snapshots.append(clock.now)
        return build_diagnostics._build_build_diagnostics_payload(context)

    def compiler_identity(path, **_):
        state.events.append("compiler_identity")
        state.identity_calls.append(path)
        clock.now += state.identity_cost
        return {"sha256": "c" * 64, "size": 128, "entrypoint": path.name}

    def analyze(**_):
        state.events.append("analysis")
        state.analyses.append(clock.now)
        clock.now += 1.0
        return {"observed_at_sec": state.analyses[-1]}

    monkeypatch.setattr(
        cli_backend_output_pipeline, "executable_content_identity", compiler_identity
    )
    monkeypatch.setattr(
        build_results, "_native_artifact_binary_image_analysis_payload", analyze
    )
    monkeypatch.setattr(
        build_results, "_non_native_artifact_binary_image_analysis_payload", analyze
    )
    monkeypatch.setattr(
        cli_backend_output_pipeline,
        "_ensure_native_runtime_lib_ready_before_link",
        lambda *_, **__: True,
    )
    binary = tmp_path / "app.exe"
    candidate = tmp_path / "candidate.exe"
    identity = object()
    binding = SimpleNamespace(
        runtime_lib=tmp_path / "runtime.lib",
        build_identity=identity,
        verify=lambda: None,
    )
    admission = BuildPythonAdmission()
    request.addfinalizer(admission.close)
    runtime = SimpleNamespace(
        build_python_admission=admission,
        native_runtime_codegen_binding=binding,
        native_runtime_build_identity=identity,
        runtime_lib=binding.runtime_lib,
        native_runtime_build_failure=None,
        revoke_native_runtime_admission=lambda: None,
    )
    cache_setup = _BackendCacheSetup(
        artifact_contract=resolve_backend_artifact_contract(
            target="native", emit_mode="bin"
        ),
        cache_enabled=True,
        cache_key="app-key",
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        cache_candidates=(),
        cache_hit=True,
        cache_hit_tier="module",
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        stdlib_object_manifest=None,
        stdlib_module_symbols=frozenset(),
    )
    config = SimpleNamespace(
        backend_cargo_profile="release",
        runtime_cargo_profile="dev-fast",
        resolved_capability_policy=CapabilityManifest().resolve(),
        capabilities_source=None,
        pgo_profile_payload=None,
        runtime_feedback_payload=None,
        cargo_timeout=None,
        link_timeout=None,
    )
    preamble = SimpleNamespace(
        diagnostics_enabled=True,
        phase_starts=phases,
        resolved_diagnostics_verbosity="summary",
        warnings=[],
        native_arch_perf_enabled=False,
    )
    layout = SimpleNamespace(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        is_wasm_freestanding=False,
        emit_mode="bin",
        output_artifact=tmp_path / "app.lib",
        output_binary=binary,
        target_triple=None,
        linked=True,
        linked_output_path=None,
        split_runtime=False,
        emit_ir_path=None,
    )
    backend_runtime = SimpleNamespace(
        runtime_state=runtime,
        ensure_runtime_wasm_both=None,
        cache_key="app-key",
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        backend_bin=tmp_path / "compiler.exe",
        backend_compiler_fingerprint="selected-compiler",
    )
    args = dict(
        prepared_build_preamble=preamble,
        prepared_build_roots=SimpleNamespace(
            molt_root=tmp_path, project_root=tmp_path, sysroot_path=None
        ),
        prepared_build_config=config,
        resolved_build_entry=SimpleNamespace(source_path=tmp_path / "app.py"),
        output_layout=layout,
        native_object_destination=None,
        prepared_backend_setup=SimpleNamespace(
            backend="native",
            cache_setup=cache_setup,
            backend_bin=backend_runtime.backend_bin,
        ),
        prepared_backend_runtime_context=backend_runtime,
        prepared_backend_compile=SimpleNamespace(
            cache_enabled=True,
            cache_hit=True,
            cache_hit_tier="module",
            backend_daemon_cached=None,
            backend_daemon_cache_tier=None,
            backend_daemon_config_digest=None,
            wasm_table_base=0,
        ),
        app_export_contract_path=None,
        native_artifact_plan=None,
        artifacts_root=tmp_path,
        resolved_modules=frozenset(),
        build_diagnostics_payload=snapshot,
        pipeline_stage_ms={},
        target="native",
        deterministic=False,
        trusted=False,
        verbose=False,
        require_linked=True,
        profile="dev",
        json_output=True,
    )

    def prepare_link(**_):
        state.events.append("link")
        # The real link boundary is also exercised on miss and reuse in
        # test_link_fingerprints; this seam supplies deterministic work costs.
        phases["link"] = clock.now
        clock.now += 5.0
        return SimpleNamespace(
            link_process=subprocess.CompletedProcess(
                ["linker"],
                7 if state.failure == "link" else 0,
                "driver output",
                "linker warning",
            ),
            link_skipped=state.link_hit,
            link_fingerprint=None,
            link_fingerprint_path=tmp_path / "link.json",
            output_binary=binary,
            link_output=binary if state.link_hit else candidate,
            output_obj=layout.output_artifact,
            stub_path=tmp_path / "main.c",
            runtime_lib=binding.runtime_lib,
            external_native_artifacts=(),
            strip_after_link=True,
            link_selection=None,
        ), None

    def finalize(**_):
        state.events.append("finalize")
        clock.now += 7.0
        return "publication refused" if state.failure == "finalize" else None

    def validate(*_):
        state.events.append("validate")
        clock.now += 7.0
        if state.failure == "validate":
            raise build_results._NativeBinaryInvalid("invalid image")

    monkeypatch.setattr(
        cli_backend_output_pipeline._link_pipeline, "_prepare_native_link", prepare_link
    )
    monkeypatch.setattr(build_results, "_finalize_native_link_candidate", finalize)
    monkeypatch.setattr(build_results, "_assert_native_binary_valid", validate)
    state.args = args
    state.snapshot = snapshot
    state.parallel = parallel
    state.binding = binding
    return state


def _terminal_payload(case, capsys):
    captured = capsys.readouterr()
    assert captured.err == ""
    messages = captured.out.splitlines()
    assert len(messages) == 1
    message = json.loads(messages[0])
    diagnostics = json.loads(case.diagnostics_file.read_text(encoding="utf-8"))
    assert message["data"]["compile_diagnostics"] == diagnostics
    assert case.snapshots == [diagnostics["total_sec"]]
    assert sum(diagnostics["phase_sec"].values()) == diagnostics["total_sec"]
    assert diagnostics["compiler"]["fingerprint"] == "selected-compiler"
    assert diagnostics["program"] == {
        "backend": case.args["prepared_backend_setup"].backend,
        "guest_profile": "dev",
        "runtime_profile": "dev-fast",
        "compiler_profile": "release",
        "target": case.args["target"],
    }
    return message, diagnostics


@pytest.mark.parametrize(
    ("link_hit", "failure", "expected_code", "expected_total"),
    [
        (False, None, 0, 23.0),
        (True, None, 0, 23.0),
        (False, "finalize", 1, 22.0),
        (True, "validate", 1, 22.0),
        (False, "link", 7, 15.0),
    ],
)
def test_native_terminal_snapshot_includes_link_and_final_result(
    terminal_build, capsys, link_hit, failure, expected_code, expected_total
):
    case = terminal_build
    case.link_hit = link_hit
    case.failure = failure
    assert (
        cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args)
        == expected_code
    )
    message, diagnostics = _terminal_payload(case, capsys)
    assert diagnostics["total_sec"] == expected_total
    assert diagnostics["phase_sec"]["link"] == 5.0
    assert (
        diagnostics["phase_attribution"]["phase_sec"]["seal"] == expected_total - 15.0
    )
    assert len(case.identity_calls) == 1
    if expected_code == 0:
        assert message["status"] == "ok"
        assert (
            diagnostics["binary_image_analysis"]["artifacts"]["observed_at_sec"] == 22.0
        )
        assert case.events[-3:] == ["analysis", "snapshot", "compiler_identity"]
    else:
        assert message["status"] == "error"
        assert not case.analyses
        expected_error = {
            "finalize": "Build failed during native finalization: publication refused",
            "validate": "Build failed: produced binary is invalid. invalid image",
            "link": "Linking failed",
        }[failure]
        assert message["errors"] == [expected_error]
    assert message["data"]["stdout"] == "driver output"
    assert message["data"]["stderr"] == "linker warning"


def test_compiler_identity_enrichment_is_outside_terminal_work_cutoff(
    terminal_build, capsys
):
    case = terminal_build
    case.identity_cost = 11.0
    assert cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args) == 0
    _, diagnostics = _terminal_payload(case, capsys)
    assert diagnostics["total_sec"] == 23.0
    assert case.clock.now == 34.0


def test_native_object_snapshot_follows_binding_and_durable_publication(
    terminal_build, tmp_path, monkeypatch, capsys
):
    case = terminal_build
    case.args["output_layout"].emit_mode = "obj"
    destination = tmp_path / "published.lib"
    source = case.args["output_layout"].output_artifact
    source.write_bytes(b"object")
    case.args["native_object_destination"] = destination
    publication = cli_backend_output_pipeline.file_publication
    real_replace = publication.durable_replace

    def prepare_object(**_):
        case.events.append("object_prepare")
        case.clock.now += 2.0
        return source, None

    def verify():
        case.events.append("binding_verify")
        case.clock.now += 3.0

    def publish(src, dst, **kwargs):
        if dst == destination:
            case.events.append("publish")
            case.clock.now += 7.0
        return real_replace(src, dst, **kwargs)

    monkeypatch.setattr(publication, "is_owned_staged_file_path", lambda *_, **__: True)
    monkeypatch.setattr(publication, "durable_replace", publish)
    monkeypatch.setattr(
        cli_backend_output_pipeline._link_pipeline,
        "_prepare_native_object_artifact",
        prepare_object,
    )
    case.binding.verify = verify
    assert cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args) == 0
    _, diagnostics = _terminal_payload(case, capsys)
    assert destination.read_bytes() == b"object"
    assert diagnostics["total_sec"] == 23.0
    assert diagnostics["phase_sec"]["seal"] == 13.0
    assert case.events == [
        "object_prepare",
        "binding_verify",
        "publish",
        "analysis",
        "snapshot",
        "compiler_identity",
    ]


@pytest.mark.parametrize(
    ("header", "header_error"),
    [(False, None), (True, None), (True, OSError), (True, ValueError)],
)
def test_wasm_terminal_snapshot_follows_requested_snapshot_header(
    terminal_build, tmp_path, monkeypatch, capsys, header, header_error
):
    case = terminal_build
    case.args["target"] = "wasm"
    case.args["prepared_backend_setup"].backend = "wasm"
    case.args["snapshot"] = header
    case.args["output_layout"].is_wasm = True

    def prepare(**_):
        case.events.append("wasm_publish")
        case.phases["wasm_publish"] = case.clock.now
        case.clock.now += 5.0
        (tmp_path / "app.wasm").write_bytes(b"published wasm")
        return SimpleNamespace(
            primary_output=tmp_path / "app.wasm",
            consumer_output=tmp_path / "app.wasm",
            bundle_root=None,
            extra_fields={},
            artifacts={},
            success_messages=[],
        ), None

    def snapshot_header(**_):
        case.events.append("snapshot_header")
        case.clock.now += 7.0
        if header_error is not None:
            raise header_error("header refused")

    monkeypatch.setattr(
        cli_backend_output_pipeline._non_native_output,
        "_prepare_non_native_build_result",
        prepare,
    )
    monkeypatch.setattr(
        cli_backend_output_pipeline._non_native_output,
        "_generate_snapshot_header",
        snapshot_header,
    )
    result = cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args)
    assert (tmp_path / "app.wasm").read_bytes() == b"published wasm"
    if header_error is not None:
        assert result == 2
        captured = capsys.readouterr()
        assert captured.err == ""
        messages = captured.out.splitlines()
        assert len(messages) == 1
        message = json.loads(messages[0])
        assert message["status"] == "error"
        assert message["errors"] == [
            "Cannot generate WASM snapshot metadata: header refused"
        ]
        assert message["data"]["returncode"] == 2
        diagnostics = json.loads(case.diagnostics_file.read_text(encoding="utf-8"))
        assert diagnostics["total_sec"] == 22.0
        assert case.snapshots == [22.0]
        assert not case.analyses
        assert len(case.identity_calls) == 1
    else:
        assert result == 0
        _, diagnostics = _terminal_payload(case, capsys)
        assert diagnostics["total_sec"] == (23.0 if header else 16.0)
        assert case.events[-3:] == ["analysis", "snapshot", "compiler_identity"]
    assert ("snapshot_header" in case.events) is header


@pytest.mark.parametrize(
    ("target", "failure"),
    [("native", False), ("native", True), ("wasm", False), ("wasm", True)],
)
def test_disabled_terminal_diagnostics_do_not_read_artifacts_or_probe_identity(
    terminal_build, monkeypatch, capsys, target, failure
):
    case = terminal_build
    case.args["prepared_build_preamble"].diagnostics_enabled = False
    if target == "native" and failure:
        case.failure = "link"
    if target == "wasm":
        case.args["target"] = "wasm"
        case.args["prepared_backend_setup"].backend = "wasm"
        case.args["output_layout"].is_wasm = True
        monkeypatch.setattr(
            cli_backend_output_pipeline._non_native_output,
            "_prepare_non_native_build_result",
            lambda **_: (
                SimpleNamespace(
                    primary_output=case.args["output_layout"].output_artifact,
                    consumer_output=case.args["output_layout"].output_artifact,
                    bundle_root=None,
                    extra_fields={},
                    artifacts={},
                    success_messages=[],
                ),
                None,
            ),
        )
        if failure:
            monkeypatch.setattr(
                cli_backend_output_pipeline._non_native_output,
                "_prepare_non_native_build_result",
                lambda **_: (
                    None,
                    output.fail("WASM output refused", True, command="build"),
                ),
            )

    def unexpected(*_, **__):
        pytest.fail("disabled diagnostics performed observational work")

    case.args["build_diagnostics_payload"] = unexpected
    monkeypatch.setattr(
        cli_backend_output_pipeline, "executable_content_identity", unexpected
    )
    monkeypatch.setattr(
        build_results, "_native_artifact_binary_image_analysis_payload", unexpected
    )
    monkeypatch.setattr(
        build_results, "_non_native_artifact_binary_image_analysis_payload", unexpected
    )
    expected = (7 if target == "native" else 2) if failure else 0
    assert (
        cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args)
        == expected
    )
    captured = capsys.readouterr()
    assert captured.err == ""
    message = json.loads(captured.out)
    assert message["status"] == ("error" if failure else "ok")
    assert "compile_diagnostics" not in message["data"]
    assert not case.diagnostics_file.exists()
    assert not case.snapshots
    assert not case.identity_calls


def test_payload_uses_one_terminal_clock_cutoff(terminal_build, monkeypatch):
    case = terminal_build
    calls = []

    def cutoff():
        calls.append(None)
        assert len(calls) == 1
        return 19.0

    monkeypatch.setattr(build_diagnostics.time, "perf_counter", cutoff)
    payload, path = build_diagnostics._build_build_diagnostics_payload(case.context)
    assert path == case.diagnostics_file
    assert payload["total_sec"] == 19.0
    assert sum(payload["phase_sec"].values()) == 19.0
    assert payload["phase_sec"]["runtime_setup"] == 11.0


@pytest.mark.parametrize("stage", ["frontend", "backend"])
@pytest.mark.parametrize("reporting_failure", [None, "file", "generation"])
def test_earlier_pipeline_failure_flushes_without_repeating_error_json(
    terminal_build, monkeypatch, capsys, stage, reporting_failure
):
    case = terminal_build
    if reporting_failure == "file":
        case.diagnostics_file.mkdir()
    elif reporting_failure == "generation":

        def refuse_projection():
            case.snapshots.append(case.clock.now)
            raise ValueError("diagnostic projection refused")

        case.snapshot = refuse_projection
    bundle = SimpleNamespace(
        prepared_frontend_run_ticket=SimpleNamespace(
            frontend_parallel_details=case.parallel
        ),
        build_diagnostics_payload=case.snapshot,
        module_graph={},
        runtime_import_dispatch_roots=set(),
        stdlib_allowlist=set(),
        spawn_enabled=False,
        output_layout=case.args["output_layout"],
        known_modules=set(),
        module_order=[],
        integration_state=None,
        record_binary_image_analysis=lambda *_, **__: None,
        artifacts_root=case.args["artifacts_root"],
        native_artifact_plan=None,
    )
    args = {
        key: case.args[key]
        for key in (
            "prepared_build_preamble",
            "prepared_build_roots",
            "prepared_build_config",
            "resolved_build_entry",
            "profile",
            "json_output",
            "target",
            "deterministic",
            "trusted",
            "verbose",
            "require_linked",
        )
    }
    args.update(
        prepared_frontend_pipeline_bundle=bundle,
        cache_dir=None,
        cache=True,
        cache_report=False,
    )

    def frontend_failure(**_):
        case.clock.now += 5.0
        return output.fail("lowering refused", True, command="build")

    def backend_failure(**_):
        case.clock.now += 5.0
        return "artifact custody refused"

    monkeypatch.setattr(cli_build_pipeline, "_run_frontend_pipeline", frontend_failure)
    monkeypatch.setattr(
        backend_pipeline,
        "_external_native_artifact_output_custody_error",
        backend_failure,
    )
    run = (
        cli_build_pipeline._run_build_pipeline
        if stage == "frontend"
        else backend_pipeline._run_backend_pipeline
    )
    assert run(**args) == 2
    captured = capsys.readouterr()
    if reporting_failure is None:
        assert captured.err == ""
    else:
        assert captured.err.startswith("Build diagnostics failed: ")
        assert captured.err.count("Build diagnostics failed: ") == 1
    messages = captured.out.splitlines()
    assert len(messages) == 1
    message = json.loads(messages[0])
    assert message["status"] == "error"
    assert message["data"]["returncode"] == 2
    assert message["errors"] == [
        "lowering refused" if stage == "frontend" else "artifact custody refused"
    ]
    if reporting_failure is None:
        diagnostics = json.loads(case.diagnostics_file.read_text(encoding="utf-8"))
        assert diagnostics["total_sec"] == 15.0
    elif reporting_failure == "file":
        assert case.diagnostics_file.is_dir()
    else:
        assert not case.diagnostics_file.exists()
    assert case.snapshots == [15.0]
    assert not case.identity_calls


def test_bolt_work_is_included_before_terminal_result(
    terminal_build, monkeypatch, capsys
):
    case = terminal_build
    case.args["bolt_requested"] = True

    def bolt(**_):
        case.events.append("bolt")
        case.clock.now += 4.0
        return 0

    monkeypatch.setattr(cli_backend_output_pipeline, "_run_bolt_post_link", bolt)
    assert cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args) == 0
    _, diagnostics = _terminal_payload(case, capsys)
    assert diagnostics["total_sec"] == 27.0
    assert diagnostics["phase_sec"]["seal"] == 12.0
    assert case.events == [
        "link",
        "bolt",
        "validate",
        "analysis",
        "snapshot",
        "compiler_identity",
    ]


def test_frontend_preparation_failure_flushes_without_repeating_error_json(
    terminal_build, monkeypatch, capsys
):
    case = terminal_build
    preamble = case.args["prepared_build_preamble"]
    preamble.midend_policy_outcomes_by_function = {}
    preamble.midend_pass_stats_by_function = {}

    def invalid_closure(_):
        case.clock.now += 5.0
        raise ValueError("rejected compile module")

    stage_bundle = (
        SimpleNamespace(with_compile_modules=invalid_closure),
        SimpleNamespace(output_layout=case.args["output_layout"]),
        SimpleNamespace(module_order=[], module_layers=[]),
        SimpleNamespace(frontend_parallel_worker_timings=[]),
        lambda *_, **__: None,
        case.snapshot,
        lambda *_, **__: None,
        lambda *_, **__: None,
        case.args["artifacts_root"],
    )
    monkeypatch.setattr(
        frontend_pipeline,
        "_prepare_frontend_stage_state",
        lambda **_: (stage_bundle, None),
    )
    monkeypatch.setattr(
        frontend_pipeline, "_dead_module_elimination_mode", lambda **_: None
    )
    args = {
        key: case.args[key]
        for key in (
            "prepared_build_preamble",
            "prepared_build_roots",
            "prepared_build_config",
            "resolved_build_entry",
            "profile",
            "json_output",
            "target",
            "trusted",
            "verbose",
            "require_linked",
        )
    }
    args.update(
        parse_codec="msgpack",
        type_hint_policy="ignore",
        fallback_policy="error",
        out_dir=None,
        split_runtime=False,
        linked=True,
        linked_output=None,
        emit=None,
        output=None,
        emit_ir=None,
        type_facts_path=None,
        tree_shake=True,
    )
    assert frontend_pipeline._prepare_frontend_pipeline(**args) == (None, 2)
    captured = capsys.readouterr()
    assert captured.err == ""
    messages = captured.out.splitlines()
    assert len(messages) == 1
    message = json.loads(messages[0])
    assert message["status"] == "error"
    assert message["data"]["returncode"] == 2
    assert message["errors"] == [
        "internal error: binary image closure plan is invalid: rejected compile module"
    ]
    diagnostics = json.loads(case.diagnostics_file.read_text(encoding="utf-8"))
    assert diagnostics["total_sec"] == 15.0
    assert case.snapshots == [15.0]
    assert not case.identity_calls


@pytest.mark.parametrize("failure", [None, "link"])
@pytest.mark.parametrize("invalid_payload", ["object", "circular", "nonfinite"])
def test_embedded_diagnostics_failure_preserves_terminal_result(
    terminal_build, monkeypatch, capsys, failure, invalid_payload
):
    case = terminal_build
    case.failure = failure
    artifact = case.args["output_layout"].output_binary
    artifact.write_bytes(b"published program")
    snapshot = case.args["build_diagnostics_payload"]

    def invalid_snapshot():
        payload, _ = snapshot()
        if invalid_payload == "object":
            payload["invalid"] = object()
        elif invalid_payload == "circular":
            payload["invalid"] = payload
        else:
            payload["invalid"] = float("nan")
        return payload, None

    case.args["build_diagnostics_payload"] = invalid_snapshot
    result = cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args)
    assert result == (7 if failure == "link" else 1)
    captured = capsys.readouterr()
    assert captured.err == ""
    messages = captured.out.splitlines()
    assert len(messages) == 1
    message = json.loads(messages[0])
    assert message["status"] == "error"
    assert message["data"]["returncode"] == result
    diagnostics_error = message["data"]["diagnostics_error"]
    assert diagnostics_error.startswith("Build diagnostics failed: ")
    assert message["errors"] == (["Linking failed"] if failure else [diagnostics_error])
    assert "compile_diagnostics" not in message["data"]
    assert "messages" not in message["data"]
    assert message["data"]["stdout"] == "driver output"
    assert message["data"]["stderr"] == "linker warning"
    assert len(case.snapshots) == 1
    assert len(case.identity_calls) == 1
    assert not case.diagnostics_file.exists()
    assert artifact.read_bytes() == b"published program"


def _refuse_terminal_reporting(case, monkeypatch, mode):
    if mode == "file":
        # Exercise the real atomic writer with a destination it cannot replace.
        case.diagnostics_file.mkdir()
        return

    def refuse_identity(path, **_):
        case.identity_calls.append(path)
        error = OSError if mode == "identity_os" else ValueError
        raise error("compiler identity unavailable")

    monkeypatch.setattr(
        cli_backend_output_pipeline, "executable_content_identity", refuse_identity
    )


@pytest.mark.parametrize("failure", ["finalize", "validate", "link"])
@pytest.mark.parametrize("reporting_failure", ["file", "identity_os", "identity_value"])
def test_native_primary_failure_survives_diagnostic_failure(
    terminal_build, monkeypatch, capsys, failure, reporting_failure
):
    case = terminal_build
    case.failure = failure
    case.link_hit = failure == "validate"
    artifact = case.args["output_layout"].output_binary
    artifact.write_bytes(b"existing published program")
    _refuse_terminal_reporting(case, monkeypatch, reporting_failure)
    result = cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args)
    assert result == (7 if failure == "link" else 1)
    captured = capsys.readouterr()
    assert captured.err == ""
    messages = captured.out.splitlines()
    assert len(messages) == 1
    message = json.loads(messages[0])
    assert message["status"] == "error"
    assert message["data"]["returncode"] == result
    assert message["errors"] == [
        {
            "finalize": "Build failed during native finalization: publication refused",
            "validate": "Build failed: produced binary is invalid. invalid image",
            "link": "Linking failed",
        }[failure]
    ]
    assert message["data"]["diagnostics_error"].startswith("Build diagnostics failed: ")
    assert message["data"]["stdout"] == "driver output"
    assert message["data"]["stderr"] == "linker warning"
    assert case.snapshots == [15.0 if failure == "link" else 22.0]
    assert len(case.identity_calls) == 1
    assert not case.analyses
    assert artifact.read_bytes() == b"existing published program"
    if reporting_failure == "file":
        assert case.diagnostics_file.is_dir()
        assert message["data"]["compile_diagnostics"]["total_sec"] == case.snapshots[0]
    else:
        assert not case.diagnostics_file.exists()
        assert "compile_diagnostics" not in message["data"]


@pytest.mark.parametrize("kind", ["native", "object", "wasm"])
@pytest.mark.parametrize("reporting_failure", ["file", "identity_os"])
def test_reporting_failure_prevents_success_and_retains_published_artifact(
    terminal_build, tmp_path, monkeypatch, capsys, kind, reporting_failure
):
    case = terminal_build
    artifact = case.args["output_layout"].output_binary
    published_bytes = b"newly published program"
    if kind == "native":
        finalize = build_results._finalize_native_link_candidate

        def publish(**kwargs):
            result = finalize(**kwargs)
            assert result is None
            artifact.write_bytes(published_bytes)
            return None

        monkeypatch.setattr(build_results, "_finalize_native_link_candidate", publish)
    elif kind == "object":
        source = case.args["output_layout"].output_artifact
        source.write_bytes(published_bytes)
        artifact = tmp_path / "published.lib"
        case.args["native_object_destination"] = artifact
        case.args["output_layout"].emit_mode = "obj"
        monkeypatch.setattr(
            cli_backend_output_pipeline.file_publication,
            "is_owned_staged_file_path",
            lambda *_, **__: True,
        )
        monkeypatch.setattr(
            cli_backend_output_pipeline._link_pipeline,
            "_prepare_native_object_artifact",
            lambda **_: (source, None),
        )
    else:
        artifact = tmp_path / "app.wasm"
        case.args["target"] = "wasm"
        case.args["prepared_backend_setup"].backend = "wasm"
        case.args["output_layout"].is_wasm = True

        def publish_wasm(**_):
            artifact.write_bytes(published_bytes)
            return SimpleNamespace(
                primary_output=artifact,
                consumer_output=artifact,
                bundle_root=None,
                extra_fields={},
                artifacts={},
                success_messages=["WASM ready"],
            ), None

        monkeypatch.setattr(
            cli_backend_output_pipeline._non_native_output,
            "_prepare_non_native_build_result",
            publish_wasm,
        )
    _refuse_terminal_reporting(case, monkeypatch, reporting_failure)
    assert cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args) == 1
    captured = capsys.readouterr()
    assert captured.err == ""
    messages = captured.out.splitlines()
    assert len(messages) == 1
    message = json.loads(messages[0])
    assert message["status"] == "error"
    assert message["data"]["returncode"] == 1
    assert "messages" not in message["data"]
    diagnostics_error = message["data"]["diagnostics_error"]
    assert diagnostics_error.startswith("Build diagnostics failed: ")
    assert message["errors"] == [diagnostics_error]
    assert message["data"]["output"] == str(artifact)
    assert len(case.snapshots) == 1
    assert len(case.identity_calls) == 1
    assert len(case.analyses) == 1
    assert artifact.read_bytes() == published_bytes
    if kind == "native":
        assert message["data"]["stdout"] == "driver output"
        assert message["data"]["stderr"] == "linker warning"
    if reporting_failure == "file":
        assert case.diagnostics_file.is_dir()
    else:
        assert not case.diagnostics_file.exists()


@pytest.mark.parametrize("failure", [None, "finalize"])
def test_text_reporting_failure_keeps_primary_error_and_avoids_success(
    terminal_build, monkeypatch, capsys, failure
):
    case = terminal_build
    case.failure = failure
    case.args["json_output"] = False
    _refuse_terminal_reporting(case, monkeypatch, "file")
    assert cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args) == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert "driver output" in captured.err
    assert "linker warning" in captured.err
    assert captured.err.count("Build diagnostics failed: ") == 1
    assert "Successfully built" not in captured.err
    if failure is not None:
        assert (
            "Build failed during native finalization: publication refused"
            in captured.err
        )
    assert len(case.snapshots) == 1


@pytest.mark.parametrize("selected", ["native", "llvm"])
def test_diagnostics_keep_requested_backend_after_ambient_selector_changes(
    terminal_build, monkeypatch, capsys, selected
):
    case = terminal_build
    case.args["prepared_backend_setup"].backend = selected
    monkeypatch.setenv("MOLT_BACKEND", "llvm" if selected == "native" else "cranelift")
    assert cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args) == 0
    _message, diagnostics = _terminal_payload(case, capsys)
    assert diagnostics["program"]["backend"] == selected
    assert diagnostics["program"]["target"] == "native"


@pytest.mark.parametrize(
    ("requested", "resolved"),
    [
        ("release", "wasm-release"),
        ("release-output", "release-output"),
        ("dev-fast", "dev-fast"),
    ],
)
def test_wasm_terminal_diagnostics_report_resolved_runtime_profile(
    terminal_build, monkeypatch, capsys, requested, resolved
):
    case = terminal_build
    monkeypatch.delenv("MOLT_WASM_CARGO_PROFILE", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_BUILD_PROFILE", raising=False)
    case.args["target"] = "wasm"
    case.args["prepared_backend_setup"].backend = "wasm"
    case.args["profile"] = "release"
    case.args["output_layout"].is_wasm = True
    case.args["prepared_build_config"].runtime_cargo_profile = requested
    monkeypatch.setattr(
        cli_backend_output_pipeline._non_native_output,
        "_prepare_non_native_build_result",
        lambda **_: (None, output.fail("WASM output refused", True, command="build")),
    )
    assert cli_backend_output_pipeline._emit_backend_pipeline_outputs(**case.args) == 2
    captured = capsys.readouterr()
    assert captured.err == ""
    message = json.loads(captured.out)
    assert message["status"] == "error"
    assert message["errors"] == ["WASM output refused"]
    diagnostics = json.loads(case.diagnostics_file.read_text(encoding="utf-8"))
    # The non-native producer has already emitted its failure envelope. The
    # terminal reporting boundary still publishes the selected facts to disk.
    assert "compile_diagnostics" not in message["data"]
    assert diagnostics["program"] == {
        "backend": "wasm",
        "guest_profile": "release",
        "compiler_profile": "release",
        "runtime_profile": resolved,
        "target": "wasm",
    }
