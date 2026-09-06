from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

from molt.cli.runtime_wasm_generation import (
    RuntimeWasmExpectedPair,
    publish_runtime_wasm_generation,
)
from tests.runtime_build_identity_helper import runtime_build_identity


def _load_wasm_link():
    path = Path(__file__).resolve().parents[1] / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link_generation", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_linker_requires_caller_trusted_atomic_pair_identity(tmp_path: Path) -> None:
    wasm_link = _load_wasm_link()
    shared_identity = runtime_build_identity("shared")
    reloc_identity = runtime_build_identity("reloc")
    shared = tmp_path / "deploy" / "molt_runtime.wasm"
    reloc = tmp_path / "deploy" / "molt_runtime_reloc.wasm"
    shared.parent.mkdir()
    source = tmp_path / "source"
    source.mkdir()
    source_shared = source / shared.name
    source_reloc = source / reloc.name
    source_shared.write_bytes(b"shared-runtime")
    source_reloc.write_bytes(b"reloc-runtime")
    generation = publish_runtime_wasm_generation(
        shared,
        reloc,
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        source_shared=source_shared,
        source_reloc=source_reloc,
    )
    expected = tmp_path / "trusted-build-state" / "expected.json"
    expected.parent.mkdir()
    RuntimeWasmExpectedPair(shared_identity, reloc_identity).write(expected)

    selected = wasm_link._verify_runtime_generation(
        reloc=generation.reloc,
        shared=generation.shared,
        generation_manifest=generation.manifest,
        expected_identity=expected,
    )
    assert selected == generation
    assert selected.reloc.name.endswith(".runtime-wasm-member")
    assert selected.shared.name.endswith(".runtime-wasm-member")

    generation.reloc.write_bytes(b"tampered")
    with pytest.raises(SystemExit, match="trusted caller identity"):
        wasm_link._verify_runtime_generation(
            reloc=generation.reloc,
            shared=generation.shared,
            generation_manifest=generation.manifest,
            expected_identity=expected,
        )
