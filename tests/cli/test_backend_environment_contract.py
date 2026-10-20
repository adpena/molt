from molt.compilation_target import admit_target
from molt.cli.cache_keys import FunctionDigests

import json
from pathlib import Path

import pytest

from molt.backend_environment import (
    DEBUG_ARTIFACT_DIR_ENV,
    CodegenSelection,
    codegen_environment_inputs,
    compilation_diagnostics_requested,
    environment_keys,
)
from molt.cli import backend_cache
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
)
from molt.cli.backend_execution import _backend_codegen_env_digest

# These cases build synthetic projects and assert developer-host roots;
# hosted custody has its own cases in tests/test_dx_run_context.py.
pytestmark = pytest.mark.usefixtures("developer_host_context")


def test_emitter_and_presence_gated_pass_inputs_invalidate_frontend_identity():
    baseline = _backend_codegen_env_digest(is_wasm=False, env={})
    for name, value in (
        ("MOLT_BACKEND_INLINE_EXC_DISABLED", "1"),
        ("MOLT_DISABLE_RC_COALESCE", ""),
        ("MOLT_DISABLE_EXC_ELIDE", "1"),
    ):
        assert _backend_codegen_env_digest(is_wasm=False, env={name: value}) != baseline
        assert (
            codegen_environment_inputs(is_wasm=False, env={name: value})[name] == value
        )
        assert name in environment_keys("common", "native")
    assert "MOLT_DISABLE_RC_COALESCING" not in environment_keys("common", "native")


def test_wasm_consumers_share_the_same_catalog():
    for name in ("MOLT_WASM_TAIL_CALL", "MOLT_WASM_NATIVE_EH", "MOLT_WASM_PROFILE"):
        assert _backend_codegen_env_digest(
            is_wasm=True, env={name: "0"}
        ) != _backend_codegen_env_digest(is_wasm=True, env={})


def test_compilation_diagnostic_presence_and_observation_policy():
    assert not compilation_diagnostics_requested({})
    for name in environment_keys("diagnostic"):
        for value in ("", "0", "1"):
            assert compilation_diagnostics_requested({name: value})
    assert not compilation_diagnostics_requested(
        {name: "1" for name in environment_keys("observation", "resource")}
    )


def test_backend_debug_artifacts_follow_the_artifact_root_not_the_checkout(
    tmp_path: Path,
):
    """HF-111: the CLI names the directory; the backend derives no root."""
    from molt.path_custody import host_path_is_within
    from molt.source_root import compiler_source_root

    artifact_root = tmp_path / "artifacts"
    rooted = CodegenSelection().environment({"MOLT_EXT_ROOT": str(artifact_root)})
    default = Path(CodegenSelection().environment({})[DEBUG_ARTIFACT_DIR_ENV])

    # The artifact root's scratch, the layout the backend used to derive.
    assert rooted[DEBUG_ARTIFACT_DIR_ENV] == str(
        artifact_root.resolve() / "tmp" / "molt-backend"
    )
    assert default.is_absolute()
    assert not host_path_is_within(default, compiler_source_root())
    # The backend no longer reads the artifact root, so no request carries it.
    assert "MOLT_EXT_ROOT" not in environment_keys(
        "common", "native", "wasm", "diagnostic", "observation", "resource"
    )
    assert DEBUG_ARTIFACT_DIR_ENV in environment_keys("observation")


def test_explicit_backend_debug_artifact_dir_wins_and_is_made_absolute(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.chdir(tmp_path)
    explicit = tmp_path / "evidence" / "backend-debug"

    kept = CodegenSelection().environment({DEBUG_ARTIFACT_DIR_ENV: str(explicit)})
    relative = CodegenSelection().environment({DEBUG_ARTIFACT_DIR_ENV: "rel/debug"})

    assert kept[DEBUG_ARTIFACT_DIR_ENV] == str(explicit)
    # A daemon with another working directory resolves the same directory.
    assert relative[DEBUG_ARTIFACT_DIR_ENV] == str(tmp_path / "rel" / "debug")
    assert not (tmp_path / "rel").exists()


@pytest.mark.parametrize("tier", ["module", "function"])
@pytest.mark.parametrize("synced_output", [False, True])
def test_compilation_diagnostics_bypass_backend_candidates_and_synced_outputs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    tier: str,
    synced_output: bool,
):
    for name in environment_keys("diagnostic", "observation"):
        monkeypatch.delenv(name, raising=False)
    # WASM reuse also needs the provisioned structural validator, which a
    # build-free unit cell lacks; this case checks only the diagnostics bypass.
    monkeypatch.setattr(
        "molt.cli.runtime_wasm_validation._reusable_wasm_artifact_validation_error",
        lambda _path: None,
    )
    candidate = tmp_path / "cached.wasm"
    wasm_bytes = b"\x00asm\x01\x00\x00\x00"
    candidate.write_bytes(wasm_bytes)
    output = tmp_path / "output.wasm"
    contract = BackendArtifactContract(BackendArtifactKind.WASM)

    def admit():
        return backend_cache._try_cached_backend_candidates(
            project_root=tmp_path,
            cache_candidates=[(tier, candidate)],
            output_artifact=output,
            artifact_contract=contract,
            cache_key="module-key" if tier == "module" else None,
            function_cache_key="function-key",
            cache_path=None,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            warnings=[],
        )

    if synced_output:
        assert admit() == (True, tier)
        assert output.read_bytes() == wasm_bytes
        candidate.unlink()
    for value in ("", "1"):
        monkeypatch.setenv("MOLT_DUMP_IR", value)
        assert admit() == (False, None)
        assert output.exists() == synced_output
    monkeypatch.delenv("MOLT_DUMP_IR")
    monkeypatch.setenv("MOLT_BACKEND_TIMING", "1")
    assert admit() == (True, tier)
    assert output.read_bytes() == wasm_bytes


def test_diagnostics_do_not_bypass_backend_transport_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("MOLT_DUMP_IR", "")
    with pytest.raises(ValueError, match="Shared stdlib extraction requires"):
        backend_cache._try_cached_backend_candidates(
            project_root=tmp_path,
            cache_candidates=[],
            output_artifact=tmp_path / "output.wasm",
            artifact_contract=BackendArtifactContract(BackendArtifactKind.WASM),
            cache_key=None,
            function_cache_key=None,
            cache_path=None,
            stdlib_object_path=tmp_path / "stdlib.a",
            stdlib_object_cache_key=None,
            warnings=[],
        )


def test_backend_rejects_unowned_inherited_guard_scratch(tmp_path):
    with pytest.raises((ValueError, OSError), match="scratch|allocation|generation"):
        CodegenSelection().environment({"MOLT_GUARD_SCRATCH_ROOT": str(tmp_path)})


def test_guard_scratch_is_not_a_request_or_codegen_identity_input():
    from molt.backend_environment import request_environment_keys

    guard = {"MOLT_GUARD_SCRATCH_ROOT": "/request-specific-scratch"}
    assert "MOLT_GUARD_SCRATCH_ROOT" not in request_environment_keys()
    assert _backend_codegen_env_digest(
        is_wasm=False, env=guard
    ) == _backend_codegen_env_digest(is_wasm=False, env={})


@pytest.mark.parametrize("output", [None, "cache.a"])
@pytest.mark.parametrize("probe", [False, True])
def test_optional_stdlib_cache_requires_distinct_owned_output(tmp_path, output, probe):
    from molt.cli.backend_execution import _backend_daemon_compile_request_bytes
    from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
    from tests.cli.native_link_test_support import transport_codegen_binding

    payload, error = _backend_daemon_compile_request_bytes(
        ir_path=None if probe else Path("backend-ir.msgpack"),
        backend_output=tmp_path / "app.a",
        artifact_contract=resolve_backend_artifact_contract(
            target=admit_target("native"), emit_mode="bin"
        ),
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        cache_key="module",
        function_cache_key=None,
        config_digest=None,
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
        stdlib_object_path=tmp_path / output if output else None,
        native_runtime_codegen_binding=transport_codegen_binding(tmp_path / "runtime"),
        request_environment={"MOLT_STDLIB_CACHE_ENTRY": str(tmp_path / "cache.a")},
    )
    assert payload is None
    assert (
        error
        == "Shared stdlib cache entry requires a distinct build-owned archive output"
    )


def _native_cache_setup(
    tmp_path: Path,
    *,
    entry: str = "main",
    profile: str = "dev",
    stdlib_modules: tuple[str, ...] = (),
):
    from molt.cli import backend_cache_setup
    from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
    from molt.cli.models import _ModuleGraphMetadata
    from molt.cli.rust_cpu_policy import RustCpuPolicy
    from molt.target_python import TargetPythonVersion

    return backend_cache_setup._prepare_backend_cache_setup(
        function_digests=FunctionDigests(),
        backend_bin=tmp_path / "backend",
        cache_enabled=True,
        ir={"functions": []},
        target=admit_target("native"),
        artifact_contract=resolve_backend_artifact_contract(
            target=admit_target("native"), emit_mode="bin"
        ),
        profile=profile,
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        emit_mode="bin",
        is_wasm=False,
        linked=False,
        wasm_layout=None,
        project_root=tmp_path,
        cache_dir=str(tmp_path / "cache"),
        output_artifact=tmp_path / "output.a",
        warnings=[],
        entry_module=entry,
        module_graph_metadata=_ModuleGraphMetadata(
            logical_source_path_by_module={},
            module_is_namespace_by_module={},
            module_is_package_by_module={},
            module_execution_kind_by_module={},
            frontend_module_costs=None,
            stdlib_like_by_module={name: True for name in stdlib_modules},
        ),
        target_python=TargetPythonVersion(3, 12, 0),
        backend_compiler_fingerprint="selected-compiler",
        codegen=CodegenSelection(),
        rust_cpu=RustCpuPolicy.BASELINE,
    )


def test_operation_cache_keys_follow_selected_inputs_and_ignore_ambient_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    def setup(entry: str = "main", profile: str = "dev"):
        return _native_cache_setup(tmp_path, entry=entry, profile=profile)

    for name in environment_keys(
        "common",
        "native",
        "native_context",
        "native_partition",
        "native_reuse",
        "build_identity",
    ):
        monkeypatch.delenv(name, raising=False)
    baseline = setup()
    for name in environment_keys(
        "native_context",
        "native_partition",
        "native_reuse",
        "transport",
        "build_identity",
        "observation",
        "resource",
    ):
        monkeypatch.setenv(name, "hostile-ambient-custody")
    # Do not feed invalid resource budgets through unrelated host admission.
    for name in environment_keys("resource"):
        monkeypatch.delenv(name)
    selected = setup()
    for name in ("cache_key", "function_cache_key", "stdlib_object_cache_key"):
        assert getattr(selected, name) == getattr(baseline, name)
        assert getattr(setup(entry="different"), name) != getattr(baseline, name)
    assert selected.backend_environment["MOLT_ENTRY_MODULE"] == "main"
    assert (
        selected.backend_environment["MOLT_BACKEND_COMPILER_FINGERPRINT"]
        == "selected-compiler"
    )
    assert selected.backend_environment["MOLT_STDLIB_MODULE_SYMBOLS"] == "[]"
    assert "MOLT_STDLIB_OBJ" not in selected.backend_environment
    sized = setup(profile="release-size")
    assert sized.backend_environment["MOLT_BACKEND_OPT_LEVEL"] == "speed_and_size"
    assert sized.cache_key != baseline.cache_key
    monkeypatch.setenv("MOLT_DUMP_CLIF", "")
    diagnostic = setup()
    assert diagnostic.cache_enabled is False
    assert diagnostic.cache_key is None and diagnostic.function_cache_key is None
    assert diagnostic.cache_path is None and diagnostic.function_cache_path is None
    # Execute-only still owns its required stdlib output for final linking.
    assert diagnostic.stdlib_object_path is not None
    assert diagnostic.stdlib_cache_entry_path is None


@pytest.mark.parametrize("is_wasm", [False, True])
def test_catalog_roles_separate_semantics_from_observation_and_transport(is_wasm):
    from molt.backend_environment import request_environment_inputs

    baseline = _backend_codegen_env_digest(is_wasm=is_wasm, env={})
    for name in environment_keys("diagnostic", "observation", "resource", "transport"):
        assert _backend_codegen_env_digest(is_wasm=is_wasm, env={name: ""}) == baseline
        assert request_environment_inputs({name: ""}) == {name: ""}
    if not is_wasm:
        absent = codegen_environment_inputs(is_wasm=False, env={})
        left = codegen_environment_inputs(
            is_wasm=False, env={"MOLT_STDLIB_OBJ": "/left.a"}
        )
        right = codegen_environment_inputs(
            is_wasm=False, env={"MOLT_STDLIB_OBJ": "/right.a"}
        )
        assert left == right == {"MOLT_STDLIB_OBJ": True}
        assert left != absent
        # Reuse constraints derive from the core variant; feeding them back
        # into it would make the stdlib key recursively depend on itself.
        assert (
            _backend_codegen_env_digest(
                is_wasm=False, env={"MOLT_STDLIB_CACHE_KEY": "derived"}
            )
            == baseline
        )


def test_stdlib_constituent_authority_binds_object_inputs_not_program_context(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    """The backend keys every stdlib archive constituent by this manifest
    without its cache_key (shared_stdlib_cache/publish/objects.rs), together
    with the constituent's own job. An added import or another entry module
    changes the program, never an unchanged constituent's machine code.
    """
    for name in environment_keys(
        "common", "native", "native_context", "native_partition", "native_reuse"
    ):
        monkeypatch.delenv(name, raising=False)

    def constituent_authority(selected) -> dict[str, object]:
        manifest = json.loads(selected.stdlib_object_manifest)
        del manifest["cache_key"]
        return manifest

    def program_context(selected) -> dict[str, str]:
        return {
            name: selected.backend_environment[name]
            for name in environment_keys("native_context")
        }

    baseline = _native_cache_setup(tmp_path, stdlib_modules=("sys", "warnings"))
    for changed in (
        _native_cache_setup(tmp_path, stdlib_modules=("shlex", "sys", "warnings")),
        _native_cache_setup(tmp_path, entry="tool", stdlib_modules=("sys", "warnings")),
    ):
        # The program's own artifacts still follow its context.
        assert program_context(changed) != program_context(baseline)
        assert changed.cache_key != baseline.cache_key
        assert changed.stdlib_object_cache_key != baseline.stdlib_object_cache_key
        assert constituent_authority(changed) == constituent_authority(baseline)

    # Settings an object job reads still separate constituents.
    monkeypatch.setenv("MOLT_BACKEND_REGALLOC_ALGORITHM", "single_pass")
    tuned = _native_cache_setup(tmp_path, stdlib_modules=("sys", "warnings"))
    assert constituent_authority(tuned) != constituent_authority(baseline)
