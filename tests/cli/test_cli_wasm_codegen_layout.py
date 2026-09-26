from __future__ import annotations

from pathlib import Path

import pytest

from molt._wasm_abi_generated import (
    WASM_RESERVED_RUNTIME_CALLABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLES,
)
from molt.cli import backend_compile
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
)
from molt.cli.models import (
    _BackendCacheSetup,
    _BackendExecutionResult,
    _RuntimeArtifactState,
)
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmCodegenBinding,
    bind_runtime_wasm_codegen,
    publish_runtime_wasm_generation,
)
from molt.cli.wasm_codegen_layout import prepare_wasm_codegen_layout
from tests.runtime_build_identity_helper import runtime_build_identity
from tests.wasm_callable_table_fixtures import _wasm_string, _wasm_u32


def _runtime_module(table_min: int, pages: int) -> bytes:
    """An actual module with an imported table/memory and a complete ABI prefix."""
    prefix = WASM_RESERVED_RUNTIME_CALLABLE_BASE + 2 * len(
        WASM_RESERVED_RUNTIME_CALLABLES
    )
    imports = (
        b"\x02"
        + _wasm_string("env")
        + _wasm_string("__indirect_function_table")
        + b"\x01\x70\x00"
        + _wasm_u32(table_min)
        + _wasm_string("env")
        + _wasm_string("memory")
        + b"\x02\x00"
        + _wasm_u32(pages)
    )
    sections = (
        (1, b"\x01\x60\x00\x00"),
        (2, imports),
        (3, b"\x01\x00"),
        (9, b"\x01\x00\x41\x01\x0b" + _wasm_u32(prefix) + bytes(prefix)),
        (10, b"\x01\x02\x00\x0b"),
    )
    return b"\x00asm\x01\x00\x00\x00" + b"".join(
        bytes([section]) + _wasm_u32(len(payload)) + payload
        for section, payload in sections
    )


def _bind_pair(
    root: Path, *, seed: str = "pair", pages: int = 2
) -> RuntimeWasmCodegenBinding:
    source = root / seed
    source.mkdir()
    shared = source / "shared.wasm"
    reloc = source / "reloc.wasm"
    shared.write_bytes(_runtime_module(256, pages))
    reloc.write_bytes(_runtime_module(128, pages + 1))
    pair = publish_runtime_wasm_generation(
        root / "molt_runtime.wasm",
        root / "molt_runtime_reloc.wasm",
        shared_identity=runtime_build_identity("shared", seed),
        reloc_identity=runtime_build_identity("reloc", seed),
        source_shared=shared,
        source_reloc=reloc,
    )
    return bind_runtime_wasm_codegen(pair, None)


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
    binding = _bind_pair(tmp_path)
    # Another caller may replace the shared selection, never this codegen pair.
    _bind_pair(tmp_path, seed="replacement", pages=7)
    for name in (
        "MOLT_WASM_DATA_BASE",
        "MOLT_WASM_TABLE_BASE",
        "MOLT_WASM_SPLIT_RUNTIME_APP_TABLE_BASE",
    ):
        monkeypatch.setenv(name, "999999")
    monkeypatch.delenv("MOLT_WASM_LINK", raising=False)
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
):
    contract = BackendArtifactContract(BackendArtifactKind.WASM)
    setup = _BackendCacheSetup(
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
        target_triple=None,
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
    )


@pytest.mark.parametrize("cache_tier", ["cold", "module", "locked-recheck"])
def test_codegen_and_all_cache_hits_preserve_runtime_layout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    cache_tier: str,
) -> None:
    binding = _bind_pair(tmp_path)
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
    binding = _bind_pair(tmp_path)
    getattr(binding.generation, member).write_bytes(b"corrupt")
    monkeypatch.setattr(
        backend_compile,
        "_prepare_backend_dispatch",
        lambda **kwargs: pytest.fail("must reject before dispatch"),
    )
    result, error = _compile(tmp_path, binding, cache_hit=True, cache_enabled=True)
    assert result is None and error == 2
    assert "bound runtime WASM generation changed" in capsys.readouterr().out


def test_missing_runtime_binding_fails_without_layout_defaults() -> None:
    with pytest.raises(ValueError, match="lacks a bound pair"):
        prepare_wasm_codegen_layout(None, linked=True, split_runtime=True)
