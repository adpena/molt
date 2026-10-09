from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace as NS

import pytest

from molt.backend_environment import CodegenSelection
from molt import file_publication
from molt.capability_manifest import CapabilityManifest
from molt.cli import (
    artifact_state,
    backend_cache,
    backend_compile,
    backend_pipeline,
    link_pipeline,
    runtime_native_build,
)
from molt.cli.artifact_sync import _artifact_sync_state_path
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.models import (
    _BackendCacheSetup,
    _BuildOutputLayout,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _PreparedBackendIR,
    _PreparedBackendSetup,
    _PreparedBuildConfig,
    _RuntimeArtifactState,
)
from molt.cli.output import fail
from molt.cli.runtime_build_python import BuildPythonAdmission
from molt.cli.installed_runtime_contract import InstalledNativeAdmission
from molt.cli.native_link_manifest import (
    native_link_dependency_manifest_path,
    _read_native_link_dependency_manifest,
)
from molt.cli.native_link_custody import observe_native_link_custody
from molt.target_python import TargetPythonVersion
from molt.toolchain_identity import stable_regular_file_identity
from tests.cli.native_link_test_support import (
    native_codegen_binding,
    write_test_native_link_manifest,
    write_test_static_archive,
)
from tests.native_artifact_fixtures import native_relocatable_object


@pytest.mark.parametrize("cache_hit", [False, True])
@pytest.mark.parametrize(
    "failure",
    [
        "none",
        "setup",
        "cache_sync",
        "compile",
        "final_capture",
        "projection",
        "object_format",
        "shared_stdlib",
        "publication",
    ],
)
def test_native_object_publication_is_one_admitted_transaction(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys,
    request: pytest.FixtureRequest,
    cache_hit: bool,
    failure: str,
) -> None:
    # Stub source lowering/toolchain execution, not the transaction under test:
    # real cache copies, final runtime admission, object validation, publication,
    # JSON output, and cleanup all remain on the production path.
    destination = tmp_path / "user.o"
    previous = b"previous user object"
    destination.write_bytes(previous)
    unrelated = tmp_path / "another-owner.o"
    unrelated.write_bytes(b"preserve unrelated output")
    backend_object = tmp_path / "backend.o"
    object_bytes = native_relocatable_object(symbols=("molt_main",))
    backend_object.write_bytes(object_bytes)
    cache_path = tmp_path / "cache.o"
    if cache_hit:
        cache_path.write_bytes(object_bytes)
    contract = resolve_backend_artifact_contract(target="native", emit_mode="obj")

    runtime_root = tmp_path / "dev-fast"
    runtime_root.mkdir()
    archive = runtime_root / "runtime.a"
    write_test_static_archive(archive)
    identity = write_test_native_link_manifest(archive)
    binding = native_codegen_binding(archive, identity)
    admission = BuildPythonAdmission()
    request.addfinalizer(admission.close)
    runtime_state = _RuntimeArtifactState(
        build_python_admission=admission,
        runtime_lib=archive,
        native_runtime_build_identity=identity,
        native_runtime_codegen_binding=binding,
    )
    monkeypatch.setattr(
        artifact_state, "_build_state_root", lambda _: tmp_path / "state"
    )
    monkeypatch.setattr(
        runtime_native_build, "_build_state_root", lambda _: tmp_path / "state"
    )
    monkeypatch.setattr(
        runtime_native_build,
        "_ensure_runtime_lib",
        lambda *args, **kwargs: pytest.fail(
            "final admission must not reselect runtime"
        ),
    )

    def mutate_binding() -> None:
        # Seed the complete admission state at the failing consumer boundary.
        # Installed-cell creation/selection is exercised by its owning suite;
        # this transaction must revoke every authority attached to the failed
        # generation, including the operation's retained installed admission.
        facts = _read_native_link_dependency_manifest(
            archive, target_triple=None, runtime_build_identity=identity
        )
        runtime_state.installed_native_admission = InstalledNativeAdmission(
            cell_id="publication-operation",
            runtime_lib=archive,
            build_identity=identity,
            archive=binding.archive,
            manifest=stable_regular_file_identity(
                native_link_dependency_manifest_path(archive),
                label="publication fixture manifest",
            ),
            callable_projection=binding.callable_symbols,
            link_facts=facts,
            custody=observe_native_link_custody(archive, facts.custody),
            callable_semantic_digest=binding.semantic_digest,
        )
        runtime_state.installed_native_admission.verify()
        binding.callable_symbols.path.write_bytes(b"molt_other\n")

    def current_identity(*args, **kwargs):
        assert destination.read_bytes() == previous
        if failure == "final_capture":
            mutate_binding()
        return identity

    monkeypatch.setattr(
        runtime_native_build, "current_native_runtime_build_identity", current_identity
    )
    original_project = link_pipeline._prepare_native_object_artifact

    def project(**kwargs):
        result = original_project(**kwargs)
        assert destination.read_bytes() == previous
        if failure == "projection":
            mutate_binding()
        return result

    monkeypatch.setattr(link_pipeline, "_prepare_native_object_artifact", project)
    original_replace = file_publication.durable_replace

    def replace(source: Path, target: Path) -> None:
        if target == destination:
            assert destination.read_bytes() == previous
            if failure == "publication":
                raise OSError("injected object publication failure")
        original_replace(source, target)

    monkeypatch.setattr(file_publication, "durable_replace", replace)
    cache_setup = _BackendCacheSetup(
        artifact_contract=contract,
        cache_enabled=True,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=cache_path,
        function_cache_path=None,
        stdlib_object_path=backend_object if failure == "shared_stdlib" else None,
        stdlib_object_cache_key=None,
        cache_candidates=(("module", cache_path),) if cache_hit else (),
        cache_hit=cache_hit,
        cache_hit_tier="module" if cache_hit else None,
    )
    backend_setup = _PreparedBackendSetup(
        runtime_state=runtime_state,
        backend_bin=tmp_path / "backend.exe",
        cache_setup=cache_setup,
        cache_hit=cache_setup.cache_hit,
        cache_hit_tier=cache_setup.cache_hit_tier,
        cache_key=cache_setup.cache_key,
        function_cache_key=cache_setup.function_cache_key,
        cache_path=cache_setup.cache_path,
        function_cache_path=cache_setup.function_cache_path,
        stdlib_object_path=cache_setup.stdlib_object_path,
        cache_candidates=list(cache_setup.cache_candidates),
        backend_compiler_fingerprint="fixture",
    )
    runtime_context = NS(
        runtime_state=runtime_state,
        cache_setup=cache_setup,
        ensure_runtime_wasm_both=None,
        backend_bin=tmp_path / "backend.exe",
        backend_compiler_fingerprint="fixture",
        cache_hit=cache_hit,
        cache_hit_tier="module" if cache_hit else None,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=cache_path,
        function_cache_path=None,
    )
    compile_result = NS(
        cache_enabled=True,
        cache_hit=cache_hit,
        cache_hit_tier="module" if cache_hit else None,
        backend_daemon_cached=False,
        backend_daemon_cache_tier=None,
        backend_daemon_config_digest=None,
        wasm_table_base=None,
    )
    stages: list[Path] = []

    def synchronize(candidate: Path) -> None:
        if cache_hit:
            assert backend_cache._materialize_cached_backend_artifact(
                tmp_path,
                cache_path,
                candidate,
                tier="module",
                source_key="module-key",
                cache_path=cache_path,
                warnings=[],
                artifact_contract=contract,
            )
        else:
            assert (
                backend_cache._stage_backend_output_and_caches(
                    tmp_path,
                    backend_object,
                    candidate,
                    cache_path=cache_path,
                    cache_key="module-key",
                    stdlib_object_cache_key=None,
                    function_cache_path=None,
                    warnings=[],
                    artifact_contract=contract,
                )
                is None
            )
        assert candidate.read_bytes() == object_bytes
        assert destination.read_bytes() == previous

    def prepare_setup(**kwargs):
        candidate = kwargs["output_artifact"]
        stages.append(candidate)
        assert candidate != destination
        assert file_publication.is_owned_staged_file_path(
            candidate, destination, purpose="native-object", suffix=".o"
        )
        if failure == "setup":
            candidate.write_bytes(b"partial setup output")
            return None, fail("injected setup failure", True, command="build")
        if cache_hit or failure == "cache_sync":
            synchronize(candidate)
        if failure == "cache_sync":
            return None, fail("injected cache-sync failure", True, command="build")
        return backend_setup, None

    def prepare_compile(**kwargs):
        candidate = kwargs["output_artifact"]
        assert candidate == stages[0]
        if not cache_hit:
            synchronize(candidate)
        if failure == "compile":
            return None, fail("injected compile failure", True, command="build")
        if failure == "object_format":
            candidate.write_bytes(b"malformed backend object")
        return compile_result, None

    monkeypatch.setattr(backend_compile, "_prepare_backend_setup", prepare_setup)
    monkeypatch.setattr(
        backend_compile,
        "_prepare_backend_runtime_context",
        lambda **kwargs: (runtime_context, None),
    )
    monkeypatch.setattr(backend_compile, "_prepare_backend_compile", prepare_compile)
    monkeypatch.setattr(
        backend_pipeline._backend_ir,
        "_prepare_backend_ir",
        lambda **kwargs: (_PreparedBackendIR(ir={"functions": []}), None),
    )
    layout = _BuildOutputLayout(
        is_wasm=False,
        is_wasm_freestanding=False,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_mlir_emit=False,
        split_runtime=False,
        linked=False,
        target_triple=None,
        emit_mode="obj",
        output_artifact=destination,
        output_binary=None,
        linked_output_path=None,
        emit_ir_path=None,
    )
    preamble = NS(
        diagnostics_enabled=False,
        phase_starts={},
        warnings=[],
        backend_daemon_config_digest=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
        native_arch_perf_enabled=False,
        resolved_diagnostics_verbosity="quiet",
    )
    config = _PreparedBuildConfig(
        pgo_profile_summary=None,
        pgo_profile_path=None,
        runtime_feedback_summary=None,
        runtime_feedback_path=None,
        pgo_hot_function_names=set(),
        pgo_hot_function_names_sorted=(),
        pgo_profile_payload=None,
        runtime_feedback_payload=None,
        cargo_timeout=None,
        backend_timeout=None,
        link_timeout=None,
        frontend_phase_timeout=None,
        backend_profile="dev",
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        resolved_capability_policy=CapabilityManifest().resolve(),
        capabilities_source=None,
        target_python=TargetPythonVersion(3, 12, 0),
        target_sys_platform=None,
        codegen=CodegenSelection(),
    )
    bundle = NS(
        prepared_frontend_run_ticket=NS(
            frontend_layer_execution_context=NS(module_graph_metadata=None)
        ),
        module_graph={},
        runtime_import_dispatch_roots=set(),
        stdlib_allowlist=set(),
        spawn_enabled=False,
        output_layout=layout,
        known_modules=set(),
        module_order=(),
        integration_state=None,
        build_diagnostics_payload=lambda: (None, None),
        record_binary_image_analysis=lambda *args: None,
        artifacts_root=tmp_path,
        native_artifact_plan=_EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    )
    result = backend_pipeline._run_backend_pipeline(
        prepared_build_preamble=preamble,
        prepared_build_roots=NS(
            project_root=tmp_path, molt_root=tmp_path, sysroot_path=None
        ),
        prepared_build_config=config,
        resolved_build_entry=NS(
            entry_module="__main__", source_path=tmp_path / "app.py"
        ),
        prepared_frontend_pipeline_bundle=bundle,
        profile="dev",
        json_output=True,
        target="native",
        cache_dir=None,
        cache=True,
        cache_report=False,
        deterministic=True,
        trusted=False,
        verbose=False,
        require_linked=False,
    )
    payload = json.loads(capsys.readouterr().out)
    assert result == (0 if failure == "none" else 2)
    assert destination.read_bytes() == (object_bytes if failure == "none" else previous)
    assert unrelated.read_bytes() == b"preserve unrelated output"
    assert len(stages) == 1
    assert not stages[0].exists()
    assert not _artifact_sync_state_path(tmp_path, stages[0]).exists()
    assert not list(tmp_path.glob(".molt-native-object-*"))
    if failure != "setup":
        assert cache_path.read_bytes() == object_bytes
    if failure == "none":
        assert payload["status"] == "ok"
        assert payload["data"]["output"] == str(destination)
        assert payload["data"]["consumer_output"] == str(destination)
        assert payload["data"]["artifacts"]["object"] == str(destination)
    else:
        assert payload["status"] == "error"
        expected_error = {
            "setup": "injected setup failure",
            "cache_sync": "injected cache-sync failure",
            "compile": "injected compile failure",
            "final_capture": "Native runtime codegen admission failed",
            "projection": "Native runtime changed during object codegen",
            "object_format": "is not a relocatable",
            "shared_stdlib": "cannot include a separately compiled stdlib",
            "publication": "Cannot publish native object output",
        }[failure]
        assert expected_error in payload["errors"][0]
        if failure in {"final_capture", "projection"}:
            assert runtime_state.native_runtime_codegen_binding is None
            assert runtime_state.native_runtime_build_identity is None
            assert runtime_state.installed_native_admission is None
