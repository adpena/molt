from __future__ import annotations

from tests.artifact_targets import artifact_target as _target

import inspect

import pytest

import molt.cli as cli
from molt.cli import build_pipeline
from molt.cli.backend_ir import _PreparedBackendIR
from molt.target_python import _SUPPORTED_TARGET_PYTHON_BY_SHORT

_BUILD_PIPELINE_NAMES = ("_run_build_pipeline",)


def test_cli_build_pipeline_authority_is_single_home() -> None:
    for name in _BUILD_PIPELINE_NAMES:
        assert hasattr(build_pipeline, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _BUILD_PIPELINE_NAMES:
        assert f"def {name}(" not in cli_source


def _capture_pipeline(
    monkeypatch,
    tmp_path,
    *,
    capture,
    layout_changes=None,
    backend="cranelift",
    factgraph=None,
):
    from dataclasses import replace
    from types import SimpleNamespace
    from molt.compilation_target import TargetKind
    from molt.cli.build_output_layout import _BuildOutputLayout

    layout = _BuildOutputLayout(
        is_wasm=False,
        is_wasm_freestanding=False,
        is_mlir_emit=False,
        split_runtime=False,
        linked=False,
        target_triple=None,
        emit_mode="bin",
        output_artifact=tmp_path / "app.o",
        output_binary=tmp_path / "app",
        linked_output_path=None,
        emit_ir_path=None,
        facts_target=_target(None),
    )
    layout = replace(layout, **(layout_changes or {}))
    calls = []
    monkeypatch.setattr(
        build_pipeline,
        "_external_native_artifact_output_custody_error",
        lambda **_: None,
    )
    monkeypatch.setattr(
        build_pipeline, "_emit_build_diagnostics_for_result", lambda result, **_: result
    )
    prepared = _PreparedBackendIR(ir={}, function_metadata={})

    def materialize(
        _frontend: object, **kwargs: object
    ) -> tuple[_PreparedBackendIR, None]:
        calls.append("frontend")
        assert kwargs["entry_module"] == "app"
        return prepared, None

    monkeypatch.setattr(
        build_pipeline._backend_ir, "materialize_tir_closure", materialize
    )

    def backend_pipeline(**kwargs: object) -> int:
        assert kwargs["prepared_backend_ir"] is prepared
        calls.append(("backend", kwargs["capture_link_inputs"]))
        return 0

    monkeypatch.setattr(
        build_pipeline._backend_pipeline, "_run_backend_pipeline", backend_pipeline
    )
    result = build_pipeline._run_build_pipeline(
        prepared_build_preamble=SimpleNamespace(
            diagnostics_enabled=False, resolved_diagnostics_verbosity="summary"
        ),
        prepared_build_roots=None,
        prepared_build_config=SimpleNamespace(
            codegen=SimpleNamespace(backend=backend),
            target_python=_SUPPORTED_TARGET_PYTHON_BY_SHORT["3.12"],
            pgo_profile_summary=None,
            runtime_feedback_summary=None,
        ),
        resolved_build_entry=SimpleNamespace(entry_module="app"),
        prepared_frontend_pipeline_bundle=SimpleNamespace(
            output_layout=layout,
            prepared_frontend_run_ticket=None,
            native_artifact_plan=None,
            build_diagnostics_payload=None,
        ),
        profile="dev",
        json_output=False,
        target=TargetKind.WASM if layout.is_wasm else TargetKind.NATIVE,
        cache_dir=None,
        cache=True,
        cache_report=False,
        deterministic=False,
        trusted=False,
        verbose=False,
        require_linked=False,
        capture_link_inputs=capture,
        fact_graph_request=factgraph,
    )
    return result, calls


@pytest.mark.parametrize("capture", [False, True])
@pytest.mark.parametrize("wasm", [False, True])
@pytest.mark.parametrize("backend", ["cranelift", "llvm"])
def test_input_capture_reaches_backend_for_linkable_outputs(
    monkeypatch, tmp_path, capture, wasm, backend
):
    result, calls = _capture_pipeline(
        monkeypatch,
        tmp_path,
        capture=capture,
        layout_changes={"is_wasm": wasm, "linked": wasm},
        backend=backend,
    )
    assert result == 0
    assert calls == ["frontend", ("backend", capture)]


@pytest.mark.parametrize(
    "changes,backend,factgraph",
    [
        ({"emit_mode": "jit"}, "cranelift", None),
        ({"emit_mode": "obj"}, "cranelift", None),
        ({"is_mlir_emit": True}, "cranelift", None),
        ({"is_wasm": True, "linked": False}, "cranelift", None),
        ({}, "cranelift", object()),
    ],
)
def test_unsupported_capture_refuses_before_compilation(
    monkeypatch, tmp_path, capsys, changes, backend, factgraph
):
    result, calls = _capture_pipeline(
        monkeypatch,
        tmp_path,
        capture=True,
        layout_changes=changes,
        backend=backend,
        factgraph=factgraph,
    )
    assert result != 0
    assert calls == []
    assert "Link input capture requires" in capsys.readouterr().err
