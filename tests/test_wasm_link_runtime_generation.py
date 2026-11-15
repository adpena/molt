from __future__ import annotations

from dataclasses import replace
from pathlib import Path

import pytest

from molt.cli.product_store import ProductStore
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmGeneration,
    RuntimeWasmReceiptRole,
    RuntimeWasmTrust,
    publish_runtime_wasm_product,
    publish_stored_runtime_wasm_generation,
    read_runtime_wasm_generation,
)
from tests.runtime_build_identity_helper import (
    runtime_build_identity,
    runtime_cargo_environment_inputs,
)
from molt.toolchain_identity import stable_regular_file_identity
from tools import wasm_link


@pytest.mark.parametrize(
    "role", ["app", "export-contract", "scanner", "native-manifest"]
)
def test_link_cli_rejects_changed_producer_inputs_before_link(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys, role: str
) -> None:
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
            "--runtime-family-digest",
            "0" * 64,
            "--runtime-receipt-role",
            "source-build",
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


def test_linker_requires_caller_trusted_atomic_pair_identity(tmp_path: Path) -> None:
    shared_identity = runtime_build_identity("shared")
    reloc_identity = runtime_build_identity("reloc")
    source = tmp_path / "source"
    source.mkdir()
    source_shared = source / "molt_runtime.wasm"
    source_reloc = source / "molt_runtime_reloc.wasm"
    source_shared.write_bytes(b"shared-runtime")
    source_reloc.write_bytes(b"reloc-runtime")
    generation = publish_stored_runtime_wasm_generation(
        ProductStore(tmp_path / "store" / "wasm-runtime"),
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        source_shared=source_shared,
        source_reloc=source_reloc,
        cargo_environment=runtime_cargo_environment_inputs(),
    )
    trust = RuntimeWasmTrust(
        shared_identity.family_digest, RuntimeWasmReceiptRole.SOURCE_BUILD
    )

    selected = wasm_link._verify_runtime_generation(
        reloc=generation.reloc,
        shared=generation.shared,
        generation_manifest=generation.manifest,
        trust=trust,
    )
    assert selected == generation
    assert selected.reloc.name.endswith(".runtime-wasm-member")
    assert selected.shared.name.endswith(".runtime-wasm-member")
    # Another family's digest never trusts this receipt.
    with pytest.raises(SystemExit, match="trusted caller identity"):
        wasm_link._verify_runtime_generation(
            reloc=generation.reloc,
            shared=generation.shared,
            generation_manifest=generation.manifest,
            trust=replace(
                trust,
                family_digest=runtime_build_identity("shared", "other").family_digest,
            ),
        )

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
    manifest_text = generation.manifest.read_text(encoding="utf-8")
    generation.manifest.write_text(
        '{"schema":"molt.runtime-wasm-generation.v2",' + manifest_text.lstrip()[1:],
        encoding="utf-8",
    )
    assert (
        read_runtime_wasm_generation(
            generation.manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        is None
    )
    generation.manifest.write_text(manifest_text, encoding="utf-8")

    generation.reloc.write_bytes(b"tampered")
    with pytest.raises(SystemExit, match="trusted caller identity"):
        wasm_link._verify_runtime_generation(
            reloc=generation.reloc,
            shared=generation.shared,
            generation_manifest=generation.manifest,
            trust=trust,
        )


@pytest.mark.parametrize("product", [False, True])
@pytest.mark.parametrize("consumer", ["linker", "benchmark"])
def test_linker_requires_caller_receipt_role_even_when_members_match(
    tmp_path, product, consumer
):
    from argparse import Namespace
    from tools import bench_wasm_link_cache

    shared_identity = runtime_build_identity("shared")
    reloc_identity = runtime_build_identity("reloc")
    source_shared = tmp_path / "shared-source.wasm"
    source_reloc = tmp_path / "reloc-source.wasm"
    source_shared.write_bytes(b"shared-runtime")
    source_reloc.write_bytes(b"reloc-runtime")
    kwargs = dict(
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        source_shared=source_shared,
        source_reloc=source_reloc,
    )
    if product:
        destination = tmp_path / "published"
        generation = publish_runtime_wasm_product(
            destination / "molt_runtime.wasm",
            destination / "molt_runtime_reloc.wasm",
            **kwargs,
        )
        wrong_role = RuntimeWasmReceiptRole.SOURCE_BUILD
    else:
        generation = publish_stored_runtime_wasm_generation(
            ProductStore(tmp_path / "store" / "wasm-runtime"),
            cargo_environment=runtime_cargo_environment_inputs(),
            **kwargs,
        )
        wrong_role = RuntimeWasmReceiptRole.INSTALLED_PRODUCT

    def read(role: RuntimeWasmReceiptRole) -> RuntimeWasmGeneration:
        if consumer == "benchmark":
            return bench_wasm_link_cache._admitted_runtime_generation(
                Namespace(
                    runtime_generation=generation.manifest,
                    runtime_family_digest=shared_identity.family_digest,
                    runtime_receipt_role=role.value,
                )
            )
        return wasm_link._verify_runtime_generation(
            shared=generation.shared,
            reloc=generation.reloc,
            generation_manifest=generation.manifest,
            trust=RuntimeWasmTrust(shared_identity.family_digest, role),
        )

    with pytest.raises(SystemExit, match="trusted.*identity"):
        read(wrong_role)
    admitted = read(generation.receipt_role)
    assert admitted.receipt_role is generation.receipt_role
    assert admitted.shared.read_bytes() == b"shared-runtime"
    assert admitted.reloc.read_bytes() == b"reloc-runtime"
