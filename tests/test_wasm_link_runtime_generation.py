from __future__ import annotations

from argparse import Namespace
from collections.abc import Sequence
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli.runtime_wasm_generation import (
    RuntimeWasmRelocAdmission,
    RuntimeWasmTrust,
    publish_runtime_wasm_product,
)
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
)
from tests.runtime_build_identity_helper import runtime_build_identity
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
            "--runtime-generation",
            str(tmp_path / "generation"),
            "--runtime-family-digest",
            "0" * 64,
            "--runtime-receipt-role",
            "source-build",
            "--runtime-receipt-sha256",
            "0" * 64,
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


def test_link_cli_requires_shared_bytes_only_for_split(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    source_shared = tmp_path / "source-shared"
    source_reloc = tmp_path / "source-reloc"
    source_shared.write_bytes(b"shared")
    source_reloc.write_bytes(b"reloc")
    generation = publish_runtime_wasm_product(
        tmp_path / "molt_runtime.wasm",
        tmp_path / "molt_runtime_reloc.wasm",
        shared_identity=runtime_build_identity("shared", "pair"),
        reloc_identity=runtime_build_identity("reloc", "pair"),
        source_shared=source_shared,
        source_reloc=source_reloc,
    )
    app = tmp_path / "app"
    app.touch()
    command = [
        "wasm_link",
        "--runtime",
        str(generation.reloc),
        *RuntimeWasmTrust.of(generation).arguments(generation.manifest),
        "--input",
        str(app),
        "--output",
        str(tmp_path / "output"),
        "--app-export-contract",
        str(tmp_path / "contract"),
        "--wasm-facts-scanner",
        str(tmp_path / "scanner"),
    ]
    monkeypatch.setattr(
        wasm_link.wasm_link_inputs, "wasm_side_module_sdk_archives", lambda: ()
    )
    monkeypatch.setattr(
        wasm_link.wasm_toolchain,
        "resolve_wasm_linker",
        lambda: SimpleNamespace(path="wasm-ld"),
    )
    handoffs: list[
        tuple[
            RuntimeWasmRelocAdmission,
            StableRegularFileIdentity | None,
            Sequence[StableRegularFileIdentity],
        ]
    ] = []

    def capture_link(
        _args: Namespace,
        *,
        runtime_admission: RuntimeWasmRelocAdmission,
        deploy_runtime: StableRegularFileIdentity | None,
        input_versions: Sequence[StableRegularFileIdentity],
        **_kwargs: object,
    ) -> int:
        handoffs.append((runtime_admission, deploy_runtime, input_versions))
        return 0

    monkeypatch.setattr(wasm_link, "_link_admitted_standard_object", capture_link)
    split_command = [
        *command,
        "--split-runtime",
        "--runtime-shared",
        str(generation.shared),
    ]
    monkeypatch.setattr(wasm_link.sys, "argv", split_command)
    assert wasm_link.main() == 0
    _, deployed_runtime, _ = handoffs.pop()
    assert deployed_runtime == generation.shared_member_identity

    generation.shared.unlink()
    assert wasm_link.main() == 1
    assert "does not match the trusted caller identity" in capsys.readouterr().err
    assert not handoffs

    monkeypatch.setattr(wasm_link.sys, "argv", command)
    assert wasm_link.main() == 0
    ((runtime_admission, deployed_runtime, input_versions),) = handoffs
    assert runtime_admission.reloc_member_identity.path == generation.reloc
    assert deployed_runtime is None
    assert set(input_versions) == {
        generation.reloc_member_identity,
        generation.receipt_identity,
    }
