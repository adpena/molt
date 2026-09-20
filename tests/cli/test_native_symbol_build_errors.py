from __future__ import annotations

from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract

import json
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from molt.capability_manifest import CapabilityManifest
from molt.cli import (
    backend_binary,
    backend_compile,
    backend_pipeline,
    frontend_pipeline,
    native_symbol_inspection,
)
from molt.cli.models import (
    _BackendCacheSetup,
    _BuildOutputLayout,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _FrontendIntegrationState,
    _ModuleGraphMetadata,
    _PreparedBackendIR,
    _PreparedBackendRuntimeContext,
    _RuntimeArtifactState,
)
from molt.cli.output import JSON_SCHEMA_VERSION
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION
from tests.cli.native_link_test_support import static_archive_bytes
from tests.native_artifact_fixtures import native_relocatable_object


@pytest.mark.parametrize("failure_phase", ["cache_setup", "compile"])
@pytest.mark.parametrize("json_output", [False, True])
def test_symbol_reader_failure_is_a_build_error_and_releases_ir_lease(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    failure_phase: str,
    json_output: bool,
) -> None:
    target = "x86_64-pc-windows-msvc"
    artifact = tmp_path / "uninspectable.a"
    artifact.write_bytes(
        static_archive_bytes(
            native_relocatable_object(target_triple=target, symbols=("molt_main",))
        )
    )
    # Reach the real reader's typed failure without launching nm or any build.
    monkeypatch.setattr(native_symbol_inspection, "_nm_candidate_binaries", lambda: [])
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_bytes(b"backend readiness fixture; never executed")
    runtime_state = _RuntimeArtifactState(runtime_lib=tmp_path / "runtime.a")
    monkeypatch.setattr(
        backend_compile,
        "_initialize_runtime_artifact_state",
        lambda **kwargs: runtime_state,
    )
    monkeypatch.setattr(
        backend_compile,
        "_stage_runtime_callable_symbols_for_native_codegen",
        lambda *args, **kwargs: ("runtime-callables", None),
    )
    monkeypatch.setattr(backend_compile, "_backend_bin_path", lambda *args: backend_bin)
    monkeypatch.setattr(
        backend_binary,
        "_ensure_backend_binary",
        lambda *args, **kwargs: backend_binary._BackendBinaryEnsureResult(
            ok=True, cache_compiler_fingerprint="test-backend"
        ),
    )
    cache_setup = _BackendCacheSetup(
        artifact_contract=resolve_backend_artifact_contract(
            target="native", emit_mode="obj", target_triple=None
        ),
        cache_enabled=True,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        cache_candidates=(),
        cache_hit=False,
        cache_hit_tier=None,
    )
    phases: list[str] = []
    leases: list[Path] = []
    lease_dir = tmp_path / "tmp" / "backend-ir-leases"
    lease_dir.mkdir(parents=True)
    unrelated_lease = lease_dir / "other-owner.json"
    unrelated_lease.write_text("other owner", encoding="utf-8")

    def fail_symbol_inspection() -> None:
        native_symbol_inspection._native_object_global_symbol_sets(
            artifact, target_triple=target
        )
        pytest.fail("missing symbol reader was silently admitted")

    def prepare_cache(**kwargs: Any) -> _BackendCacheSetup:
        phases.append("cache_setup")
        if failure_phase == "cache_setup":
            fail_symbol_inspection()
        return cache_setup

    def prepare_compile(**kwargs: Any) -> None:
        phases.append("compile")
        lease = kwargs["_ensure_backend_ir_file_path"]()
        assert lease.is_file()
        assert json.loads(lease.read_text(encoding="utf-8")) == {"functions": []}
        assert kwargs["_ensure_backend_ir_file_path"]() == lease
        leases.append(lease)
        fail_symbol_inspection()

    monkeypatch.setattr(
        backend_compile._backend_cache_setup,
        "_prepare_backend_cache_setup",
        prepare_cache,
    )
    monkeypatch.setattr(backend_compile, "_prepare_backend_compile", prepare_compile)
    monkeypatch.setattr(
        backend_compile,
        "_prepare_backend_runtime_context",
        lambda **kwargs: (
            _PreparedBackendRuntimeContext(
                runtime_state=runtime_state,
                backend_bin=backend_bin,
                runtime_lib=runtime_state.runtime_lib,
                ensure_runtime_wasm_both=lambda required=None: True,
                cache_setup=cache_setup,
                cache_hit=False,
                cache_hit_tier=None,
                cache_key=cache_setup.cache_key,
                function_cache_key=None,
                cache_path=None,
                function_cache_path=None,
                stdlib_object_path=None,
            ),
            None,
        ),
    )
    monkeypatch.setattr(
        backend_pipeline._backend_ir,
        "_prepare_backend_ir",
        lambda **kwargs: (_PreparedBackendIR(ir={"functions": []}), None),
    )

    def unexpected_output(**kwargs: Any) -> None:
        pytest.fail("build output publication must not run after symbol failure")

    monkeypatch.setattr(
        backend_pipeline._backend_output_pipeline,
        "_emit_backend_pipeline_outputs",
        unexpected_output,
    )
    metadata = _ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        module_execution_kind_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module=None,
    )
    layout = _BuildOutputLayout(
        is_wasm=False,
        is_wasm_freestanding=False,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_mlir_emit=False,
        split_runtime=False,
        linked=False,
        target_triple=target,
        emit_mode="bin",
        output_artifact=tmp_path / "output.lib",
        output_binary=tmp_path / "app.exe",
        linked_output_path=None,
        emit_ir_path=None,
    )
    # Only upstream frontend preparation is substituted. Both build-error
    # boundaries, normal output formatting, and IR lease cleanup remain real.
    ticket = SimpleNamespace(
        frontend_layer_execution_context=SimpleNamespace(module_graph_metadata=metadata)
    )
    bundle = frontend_pipeline._PreparedFrontendPipelineBundle(
        prepared_frontend_run_ticket=ticket,
        module_graph={},
        runtime_import_dispatch_roots=set(),
        stdlib_allowlist=set(),
        spawn_enabled=False,
        output_layout=layout,
        known_modules=set(),
        module_order=(),
        integration_state=_FrontendIntegrationState(functions=[], known_classes={}),
        build_diagnostics_payload=lambda: (None, None),
        record_binary_image_analysis=lambda *args, **kwargs: None,
        artifacts_root=tmp_path,
        native_artifact_plan=_EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    )
    preamble = SimpleNamespace(
        diagnostics_enabled=False,
        warnings=[],
        phase_starts={},
        backend_daemon_config_digest=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
    )
    config = SimpleNamespace(
        pgo_hot_function_names=set(),
        frontend_phase_timeout=None,
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        cargo_timeout=None,
        backend_timeout=None,
        resolved_capability_policy=CapabilityManifest().resolve(),
    )
    result = backend_pipeline._run_backend_pipeline(
        prepared_build_preamble=preamble,
        prepared_build_roots=SimpleNamespace(project_root=tmp_path, molt_root=tmp_path),
        prepared_build_config=config,
        resolved_build_entry=SimpleNamespace(entry_module="__main__"),
        prepared_frontend_pipeline_bundle=bundle,
        profile="dev",
        json_output=json_output,
        target="native",
        cache_dir=None,
        cache=True,
        cache_report=False,
        deterministic=True,
        trusted=False,
        verbose=False,
        require_linked=False,
    )
    captured = capsys.readouterr()
    assert result == 2
    expected_message = (
        f"Cannot inspect native symbols for {artifact}: "
        "no nm/llvm-nm candidate is available"
    )
    if json_output:
        assert captured.err == ""
        assert json.loads(captured.out) == {
            "schema_version": JSON_SCHEMA_VERSION,
            "command": "build",
            "status": "error",
            "data": {"returncode": 2},
            "warnings": [],
            "errors": [expected_message],
        }
    else:
        assert captured.out == ""
        assert captured.err == expected_message + "\n"
    assert phases == (
        ["cache_setup"]
        if failure_phase == "cache_setup"
        else ["cache_setup", "compile"]
    )
    assert len(leases) == (0 if failure_phase == "cache_setup" else 1)
    assert all(not path.exists() for path in leases)
    assert list(lease_dir.iterdir()) == [unrelated_lease]
    assert unrelated_lease.read_text(encoding="utf-8") == "other owner"
    assert not layout.output_binary.exists()
    assert not layout.output_artifact.exists()
