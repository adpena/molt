from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

import pytest
from tests.wasm_linked_runner import _run_wasm_test_process

ROOT = Path(__file__).resolve().parents[1]


def _wasm_from_wat(tmp_path: Path, name: str, wat: str) -> Path:
    wasm_tools = shutil.which("wasm-tools")
    if wasm_tools is None:
        pytest.skip("wasm-tools is required for runner table-base parser tests")
    wat_path = tmp_path / f"{name}.wat"
    wasm_path = tmp_path / f"{name}.wasm"
    wat_path.write_text(wat, encoding="utf-8")
    result = _run_wasm_test_process(
        [wasm_tools, "parse", str(wat_path), "-o", str(wasm_path)],
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    return wasm_path


def _extract_table_base(wasm_path: Path) -> int | None:
    script = (
        "const fs = require('fs');"
        "const runner = require('./wasm/run_wasm.js');"
        "const value = runner.extractWasmTableBase(fs.readFileSync(process.argv[1]));"
        "process.stdout.write(JSON.stringify(value));"
    )
    result = _run_wasm_test_process(
        ["node", "-e", script, str(wasm_path)],
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
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


def test_extract_wasm_table_base_ignores_runtime_prefix_when_app_segment_exists(
    tmp_path: Path,
) -> None:
    wasm_path = _wasm_from_wat(
        tmp_path,
        "runtime_prefix_and_app_segment",
        """
        (module
          (table 4097 funcref)
          (func $runtime)
          (func $app)
          (elem (i32.const 1) func $runtime)
          (elem (i32.const 4096) func $app)
          (func (export "molt_table_init"))
        )
        """,
    )

    assert _extract_table_base(wasm_path) == 4096


def test_direct_runner_delegates_app_table_init_to_main_wrapper() -> None:
    runner = (ROOT / "wasm" / "run_wasm.js").read_text(encoding="utf-8")

    assert "hasExportedTableRefs(outputInstance)" not in runner
    assert (
        "skipping molt_table_init because exported table refs are available"
        not in runner
    )
    assert "MOLT_WASM_SKIP_TABLE_INIT" not in runner
    assert "[molt wasm] direct: call molt_table_init" not in runner
    assert (
        "App-owned slots are installed by the exported molt_main wrapper"
        in runner
    )
    assert "if (installTableRefsEnabled) {" in runner
    assert runner.index("if (installTableRefsEnabled) {") < runner.index(
        "installTableRefs(outputInstance, table, 'output');"
    )
