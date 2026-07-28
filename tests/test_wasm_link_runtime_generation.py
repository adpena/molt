from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from molt.cli.runtime_wasm_generation import publish_runtime_wasm_generation


def _digest(value: object) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def _identity(kind: str, pair: dict[str, object]) -> RuntimeBuildIdentity:
    member = {"kind": kind}
    payload = {"pair": pair, "member": member}
    return RuntimeBuildIdentity(_digest(payload), _digest(pair), payload)


def _load_wasm_link():
    path = Path(__file__).resolve().parents[1] / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link_generation", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_linker_requires_caller_trusted_atomic_pair_identity(tmp_path: Path) -> None:
    wasm_link = _load_wasm_link()
    pair = {"schema": "molt.runtime-build-pair.v2", "plan": "exact"}
    shared_identity = _identity("shared", pair)
    reloc_identity = _identity("reloc", pair)
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
    expected.write_text(
        json.dumps(
            {
                "schema": "molt.runtime-wasm-expected-pair.v1",
                "shared": shared_identity.to_dict(),
                "reloc": reloc_identity.to_dict(),
            }
        ),
        encoding="utf-8",
    )

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
