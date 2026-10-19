from __future__ import annotations

from pathlib import Path

import pytest

from molt.backend_environment import CodegenSelection
from molt.cli import backend_compile
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
)
from molt.cli.models import _BackendCacheSetup, _RuntimeArtifactState
from molt.cli.backend_compile import _BackendExecutionResult
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmCodegenBinding,
    bind_runtime_wasm_codegen,
)
from molt.cli.wasm_codegen_layout import WasmCodegenLayout, prepare_wasm_codegen_layout
from molt.cli.rust_cpu_policy import RustCpuPolicy
from tests.runtime_build_identity_helper import runtime_wasm_codegen_binding_fixture


@pytest.mark.parametrize(
    ("linked", "split_runtime", "pages", "table_base"),
    [
        (True, True, 2, 1),
        (True, False, 3, 256),
        (False, False, 2, 256),
    ],
)
def test_layout_uses_bound_pair_not_mutable_selection_or_ambient_overrides(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    linked: bool,
    split_runtime: bool,
    pages: int,
    table_base: int,
) -> None:
    binding = runtime_wasm_codegen_binding_fixture(tmp_path)
    # Another caller may replace the shared selection, never this codegen pair.
    runtime_wasm_codegen_binding_fixture(tmp_path, seed="replacement", pages=7)
    for name in (
        "MOLT_WASM_DATA_BASE",
        "MOLT_WASM_TABLE_BASE",
        "MOLT_WASM_SPLIT_RUNTIME_APP_TABLE_BASE",
    ):
        monkeypatch.setenv(name, "999999")
    monkeypatch.delenv("MOLT_WASM_RELOCATABLE", raising=False)
    layout = prepare_wasm_codegen_layout(
        binding, linked=linked, split_runtime=split_runtime
    )
    assert layout.data_base == 64 * 1024 * 1024 + pages * 65536
    assert layout.table_base == table_base
    assert layout.split_app_table_base == (256 if split_runtime else None)
    assert layout.relocatable is linked
    assert layout.backend_environment()["MOLT_WASM_TABLE_BASE"] == str(table_base)


def _compile(
    root: Path,
    binding: RuntimeWasmCodegenBinding,
    *,
    cache_hit: bool,
    cache_enabled: bool,
    layout: WasmCodegenLayout | None = None,
):
    if layout is None:
        layout = prepare_wasm_codegen_layout(binding, linked=True, split_runtime=True)
    contract = BackendArtifactContract(BackendArtifactKind.WASM)
    setup = _BackendCacheSetup(
        backend_environment=layout.backend_environment(),
        wasm_layout=layout,
        artifact_contract=contract,
        cache_enabled=cache_enabled,
        cache_key=None,
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        cache_candidates=(),
        cache_hit=cache_hit,
        cache_hit_tier="module" if cache_hit else None,
    )
    backend = root / "backend"
    backend.write_bytes(b"test-process-boundary")
    return backend_compile._prepare_backend_compile(
        diagnostics_enabled=False,
        phase_starts={},
        cache_report=False,
        verbose=False,
        json_output=True,
        cache_setup=setup,
        cache_hit=cache_hit,
        cache_hit_tier=setup.cache_hit_tier,
        cache_key=None,
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        project_root=root,
        warnings=[],
        is_rust_transpile=False,
        is_wasm=True,
        split_runtime=True,
        output_artifact=root / "output.wasm",
        linked=True,
        deterministic=True,
        profile="dev",
        runtime_state=_RuntimeArtifactState(runtime_wasm_codegen_binding=binding),
        cargo_timeout=1.0,
        molt_root=root,
        backend_cargo_profile="dev-fast",
        backend_timeout=1.0,
        backend_daemon_config_digest=None,
        entry_module="main",
        artifacts_root=root,
        ir={"functions": []},
        _ensure_backend_ir_file_path=lambda: root / "ir.json",
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
        backend_bin=backend,
        codegen=CodegenSelection(),
        rust_cpu=RustCpuPolicy.BASELINE,
    )


@pytest.mark.parametrize("cache_tier", ["cold", "module", "locked-recheck"])
def test_codegen_and_all_cache_hits_preserve_runtime_layout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    cache_tier: str,
) -> None:
    binding = runtime_wasm_codegen_binding_fixture(tmp_path)
    emitted = []
    monkeypatch.setattr(backend_compile, "_backend_daemon_enabled", lambda: False)
    monkeypatch.setattr(
        backend_compile,
        "_try_cached_backend_candidates",
        lambda **kwargs: (True, "function"),
    )

    def execute(**kwargs):
        emitted.append(kwargs["backend_env"])
        return _BackendExecutionResult(None, None, None), None

    monkeypatch.setattr(backend_compile, "_execute_backend_compile", execute)
    for name in (
        "MOLT_WASM_DATA_BASE",
        "MOLT_WASM_TABLE_BASE",
        "MOLT_WASM_SPLIT_RUNTIME_APP_TABLE_BASE",
    ):
        monkeypatch.setenv(name, "999999")
    result, error = _compile(
        tmp_path,
        binding,
        cache_hit=cache_tier == "module",
        cache_enabled=cache_tier != "cold",
    )
    assert error is None
    assert result is not None and result.wasm_table_base == 1
    assert result.cache_hit is (cache_tier != "cold")
    if cache_tier == "cold":
        assert len(emitted) == 1
        assert emitted[0]["MOLT_WASM_TABLE_BASE"] == "1"
        assert emitted[0]["MOLT_WASM_SPLIT_RUNTIME_APP_TABLE_BASE"] == "256"
        assert emitted[0]["MOLT_WASM_DATA_BASE"] == str(64 * 1024 * 1024 + 2 * 65536)
    else:
        assert not emitted


@pytest.mark.parametrize("member", ["manifest", "shared", "reloc"])
def test_warm_cache_rejects_corrupt_bound_generation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    member: str,
) -> None:
    binding = runtime_wasm_codegen_binding_fixture(tmp_path)
    layout = prepare_wasm_codegen_layout(binding, linked=True, split_runtime=True)
    getattr(binding.generation, member).write_bytes(b"corrupt")
    monkeypatch.setattr(
        backend_compile,
        "_prepare_backend_dispatch",
        lambda **kwargs: pytest.fail("must reject before dispatch"),
    )
    result, error = _compile(
        tmp_path, binding, cache_hit=True, cache_enabled=True, layout=layout
    )
    assert result is None and error == 2
    assert "bound runtime WASM generation changed" in capsys.readouterr().out


def test_missing_runtime_binding_fails_without_layout_defaults() -> None:
    with pytest.raises(ValueError, match="lacks a bound pair"):
        prepare_wasm_codegen_layout(None, linked=True, split_runtime=True)


def test_layout_retains_compact_facts_and_reads_admitted_bytes_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import toolchain_identity

    binding = runtime_wasm_codegen_binding_fixture(tmp_path)
    original = toolchain_identity.read_stable_regular_file
    reads = []

    def read(identity, **kwargs):
        reads.append(identity.path)
        return original(identity, **kwargs)

    monkeypatch.setattr(toolchain_identity, "read_stable_regular_file", read)
    first = prepare_wasm_codegen_layout(binding, linked=True, split_runtime=True)
    repinned = bind_runtime_wasm_codegen(binding.generation, None)
    assert (
        prepare_wasm_codegen_layout(repinned, linked=True, split_runtime=True) == first
    )
    assert reads == [binding.generation.shared]
    for facts in (binding.generation.facts(),):
        assert all(
            not isinstance(value, (bytes, bytearray, memoryview))
            for value in (getattr(facts, field) for field in facts.__dataclass_fields__)
        )
