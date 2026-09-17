"""Linked WASM consumes the same codec capsule as native differential testing."""

import runpy
from pathlib import Path

import pytest

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_codec_parity(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    source = root / "tests/differential/basic/codec_parity.py"
    runpy.run_path(str(source), run_name="__main__")
    reference = capsys.readouterr()
    assert not reference.err

    output_wasm = build_wasm_linked(root, source, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == reference.out.strip()
