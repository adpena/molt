from __future__ import annotations

from pathlib import Path

import pytest

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
