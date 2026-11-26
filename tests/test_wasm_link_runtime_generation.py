from __future__ import annotations

import os
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
    "role",
    [
        "app",
        "export-contract",
        "scanner",
        "native-manifest",
        pytest.param(
            "linker-alias",
            marks=pytest.mark.skipif(os.name == "nt", reason="POSIX linker symlink"),
        ),
    ],
)
def test_link_cli_rejects_changed_producer_inputs_before_link(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys, role: str
) -> None:
    source = tmp_path / role
    source.write_bytes(b"admitted")
    digest = stable_regular_file_identity(source, label="test input").sha256
    linker_options: tuple[str, ...] = ()
    if role == "linker-alias":
        replacement = tmp_path / "replacement-wasm-ld"
        replacement.write_bytes(b"replacement")
        alias = tmp_path / "wasm-ld"
        alias.symlink_to(source)
        linker_options = ("--wasm-linker", str(alias), str(source))
        alias.unlink()
        alias.symlink_to(replacement)
    else:
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
            *linker_options,
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
        lambda _compiler_root: SimpleNamespace(path="wasm-ld"),
    )
    linker = tmp_path / ("wasm-ld.exe" if os.name == "nt" else "wasm-ld")
    linker.write_bytes(b"producer-selected linker")
    linker_identity = stable_regular_file_identity(linker, label="test WASM linker")
    handoffs: list[
        tuple[
            RuntimeWasmRelocAdmission,
            StableRegularFileIdentity | None,
            Sequence[StableRegularFileIdentity],
        ]
    ] = []

    def capture_link(
        args: Namespace,
        *,
        runtime_admission: RuntimeWasmRelocAdmission,
        deploy_runtime: StableRegularFileIdentity | None,
        input_versions: Sequence[StableRegularFileIdentity],
        **kwargs: object,
    ) -> int:
        if args.wasm_linker is not None:
            assert kwargs["wasm_ld"] == linker
            assert kwargs["wasm_ld_identity"] == linker_identity
        handoffs.append((runtime_admission, deploy_runtime, input_versions))
        return 0

    monkeypatch.setattr(wasm_link, "_link_admitted_standard_object", capture_link)
    split_command = [
        *command,
        "--split-runtime",
        "--runtime-shared",
        str(generation.shared),
        "--wasm-linker",
        str(linker),
        str(linker_identity.path),
        "--expected-input",
        str(linker_identity.path),
        linker_identity.sha256,
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
