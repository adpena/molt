from __future__ import annotations

import importlib.util
from dataclasses import replace
from pathlib import Path

import pytest

from molt.cli.runtime_wasm_generation import (
    RuntimeWasmExpectedPair,
    publish_runtime_wasm_generation,
    read_runtime_wasm_generation,
)
from tests.runtime_build_identity_helper import runtime_build_identity
from molt.toolchain_identity import stable_regular_file_identity


@pytest.mark.parametrize(
    "role", ["app", "export-contract", "scanner", "native-manifest"]
)
def test_link_cli_rejects_changed_producer_inputs_before_link(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys, role: str
) -> None:
    wasm_link = _load_wasm_link()
    source = tmp_path / role
    source.write_bytes(b"admitted")
    digest = stable_regular_file_identity(source, label="test input").sha256
    source.write_bytes(b"tampered")
    output = tmp_path / "output.wasm"
    output.write_bytes(b"previous valid generation")
    monkeypatch.setattr(
        wasm_link.sys,
        "argv",
        [
            "wasm_link",
            "--runtime",
            str(tmp_path / "runtime"),
            "--runtime-shared",
            str(tmp_path / "shared"),
            "--runtime-generation",
            str(tmp_path / "generation"),
            "--runtime-expected-identity",
            str(tmp_path / "expected"),
            "--input",
            str(tmp_path / "app"),
            "--output",
            str(output),
            "--app-export-contract",
            str(tmp_path / "contract"),
            "--wasm-facts-scanner",
            str(tmp_path / "scanner"),
            "--expected-input",
            str(source),
            digest,
        ],
    )
    assert wasm_link.main() == 1
    assert "changed after producer admission" in capsys.readouterr().err
    assert output.read_bytes() == b"previous valid generation"


@pytest.mark.parametrize(
    "role", ["scanner", "plan", "generation", "receipt-request", "timings"]
)
def test_complete_link_output_family_cannot_alias_inputs(
    tmp_path: Path, capsys, role: str
) -> None:
    from molt.cli.source_extension_link_requirements import (
        SourceExtensionLinkRequirements,
        source_extension_link_file,
    )
    from molt.link_outputs import link_selection_path

    wasm_link = _load_wasm_link()
    native = tmp_path / "native.o"
    native.write_bytes(b"native input")
    linked = tmp_path / "linked.wasm"
    selection = link_selection_path(linked)
    selection.write_bytes(b"protected input")
    result = wasm_link._run_wasm_ld(
        "unused-wasm-ld",
        tmp_path / "runtime",
        tmp_path / "app",
        linked,
        runtime_role="reloc",
        wasm_facts_scanner=selection if role == "scanner" else tmp_path / "scanner",
        native_link_requirements=SourceExtensionLinkRequirements(
            "wasm32-wasip1", (source_extension_link_file(native),)
        ),
        additional_inputs=(selection,) if role not in {"scanner", "timings"} else (),
        phase_timings_file=selection if role == "timings" else None,
    )
    assert result == 1
    assert "alias" in capsys.readouterr().err
    assert selection.read_bytes() == b"protected input"
    assert not linked.exists()


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
    assert selected.receipt_identity == stable_regular_file_identity(
        generation.manifest, label="link generation receipt"
    )
    assert generation.receipt_identity is None
    assert replace(selected, receipt_identity=None) == generation
    assert selected.reloc.name.endswith(".runtime-wasm-member")
    assert selected.shared.name.endswith(".runtime-wasm-member")

    original_stat = generation.reloc.stat()
    replacement = tmp_path / "replacement-runtime"
    replacement.write_bytes(b"other-runtime")
    assert replacement.stat().st_size == original_stat.st_size
    replacement.replace(generation.reloc)
    generation.reloc.touch()
    with pytest.raises(OSError, match="trusted|remained mutable"):
        wasm_link._snapshot_link_input(
            generation.reloc,
            tmp_path / "snapshot",
            label="trusted-runtime",
            attempts=1,
            retry_delay_seconds=0,
            expected_identity=selected.reloc_member_identity,
        )

    generation.reloc.write_bytes(b"reloc-runtime")
    refreshed = publish_runtime_wasm_generation(
        shared,
        reloc,
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        source_shared=source_shared,
        source_reloc=source_reloc,
    )
    manifest_text = refreshed.manifest.read_text(encoding="utf-8")
    refreshed.manifest.write_text(
        '{"schema":"molt.runtime-wasm-generation.v2",' + manifest_text.lstrip()[1:],
        encoding="utf-8",
    )
    assert (
        read_runtime_wasm_generation(
            refreshed.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is None
    )
    refreshed.manifest.write_text(manifest_text, encoding="utf-8")

    refreshed.reloc.write_bytes(b"tampered")
    with pytest.raises(SystemExit, match="trusted caller identity"):
        wasm_link._verify_runtime_generation(
            reloc=generation.reloc,
            shared=generation.shared,
            generation_manifest=generation.manifest,
            expected_identity=expected,
        )
