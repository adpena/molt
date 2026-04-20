from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _wasm_from_wat(tmp_path: Path, name: str, wat: str) -> Path:
    wasm_tools = shutil.which("wasm-tools")
    if wasm_tools is None:
        pytest.skip("wasm-tools is required for runner table-base parser tests")
    wat_path = tmp_path / f"{name}.wat"
    wasm_path = tmp_path / f"{name}.wasm"
    wat_path.write_text(wat, encoding="utf-8")
    subprocess.run(
        [wasm_tools, "parse", str(wat_path), "-o", str(wasm_path)],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return wasm_path


def _extract_table_base(wasm_path: Path) -> int | None:
    script = (
        "const fs = require('fs');"
        "const runner = require('./wasm/run_wasm.js');"
        "const value = runner.extractWasmTableBase(fs.readFileSync(process.argv[1]));"
        "process.stdout.write(JSON.stringify(value));"
    )
    result = subprocess.run(
        ["node", "-e", script, str(wasm_path)],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def test_extract_wasm_table_base_prefers_table_init_over_active_segments(
    tmp_path: Path,
) -> None:
    wasm_path = _wasm_from_wat(
        tmp_path,
        "table_init_over_active",
        """
        (module
          (table 64 funcref)
          (func $f)
          (elem (i32.const 32) func $f)
          (func (export "molt_table_init")
            i32.const 4096
            drop)
        )
        """,
    )

    assert _extract_table_base(wasm_path) == 4096


def test_extract_wasm_table_base_accepts_active_base_one(tmp_path: Path) -> None:
    wasm_path = _wasm_from_wat(
        tmp_path,
        "active_base_one",
        """
        (module
          (table 4 funcref)
          (func $f)
          (elem (i32.const 1) func $f)
        )
        """,
    )

    assert _extract_table_base(wasm_path) == 1


def test_extract_wasm_table_base_does_not_infer_runtime_prefix_for_empty_table_init(
    tmp_path: Path,
) -> None:
    wasm_path = _wasm_from_wat(
        tmp_path,
        "empty_table_init",
        """
        (module
          (table 4 funcref)
          (func $f)
          (elem (i32.const 1) func $f)
          (func (export "molt_table_init"))
        )
        """,
    )

    assert _extract_table_base(wasm_path) is None
