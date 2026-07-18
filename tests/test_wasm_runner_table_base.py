from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

import pytest
from molt._wasm_abi_generated import (
    WASM_RESERVED_RUNTIME_CALLABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLES,
)
from tests.wasm_linked_runner import _run_wasm_test_process
from tests.wasm_import_fixtures import build_wasm_tag_import_before_memory

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_WASM_APP_TABLE_BASE = 256
TEST_SHARED_WASM_TABLE_BASE = 4096


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
    cargo = shutil.which("cargo")
    if cargo is None:
        pytest.skip("cargo is required for Rust-owned callable-table publication")
    published = wasm_path.with_suffix(".published.wasm")
    result = _run_wasm_test_process(
        [
            cargo,
            "run",
            "--quiet",
            "--locked",
            "-p",
            "molt-backend",
            "--features",
            "wasm-backend",
            "--bin",
            "molt-backend",
            "--",
            "--publish-wasm-link-facts",
            str(wasm_path),
            "--output",
            str(published),
        ],
        cwd=ROOT,
        env=os.environ,
        timeout=300,
    )
    assert result.returncode == 0, result.stderr
    published.replace(wasm_path)
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


def test_parse_wasm_imports_handles_tag_imports_before_memory(tmp_path: Path) -> None:
    wasm_path = tmp_path / "tag_import_before_memory.wasm"
    wasm_path.write_bytes(build_wasm_tag_import_before_memory())
    script = (
        "const fs = require('fs');"
        "const runner = require('./wasm/run_wasm.js');"
        "const imports = runner.parseWasmImports(fs.readFileSync(process.argv[1]));"
        "process.stdout.write(JSON.stringify(imports));"
    )
    result = _run_wasm_test_process(
        ["node", "-e", script, str(wasm_path)],
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr

    parsed = json.loads(result.stdout)
    assert parsed["memory"] == {"min": 1, "max": None}
    assert parsed["funcImports"] == []
    assert parsed["tagImports"] == [
        {
            "module": "env",
            "name": "molt_exception",
            "attribute": 0,
            "typeIndex": 0,
            "parameters": [],
            "results": [],
        }
    ]


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
        )
        """,
    )

    assert _extract_table_base(wasm_path) == 4096


def test_linked_runner_uses_env_table_base_without_calling_setter(
    tmp_path: Path,
) -> None:
    wasm_path = _wasm_from_wat(
        tmp_path,
        "linked_trapping_table_base_setter",
        """
        (module
          (memory (export "molt_memory") 1)
          (table (export "molt_table") 8 funcref)
          (func $ref)
          (elem (i32.const 4) func $ref)
          (func (export "molt_set_wasm_table_base") (param i64)
            unreachable)
          (func (export "molt_main"))
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(wasm_path)],
        cwd=ROOT,
        env={**os.environ, "NODE_NO_WARNINGS": "1"},
        timeout=30,
    )
    assert result.returncode == 0, result.stderr


def test_linked_runner_calls_host_init_before_isolate_bootstrap(
    tmp_path: Path,
) -> None:
    wasm_path = _wasm_from_wat(
        tmp_path,
        "linked_host_init_before_bootstrap",
        """
        (module
          (memory (export "molt_memory") 1)
          (global $ready (mut i32) (i32.const 0))
          (func (export "molt_host_init")
            i32.const 1
            global.set $ready)
          (func (export "molt_isolate_bootstrap") (result i64)
            global.get $ready
            i32.eqz
            if
              unreachable
            end
            i64.const 0)
          (func (export "molt_main"))
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(wasm_path)],
        cwd=ROOT,
        env={**os.environ, "NODE_NO_WARNINGS": "1"},
        timeout=30,
    )
    assert result.returncode == 0, result.stderr


def test_direct_split_runner_accepts_runtime_memory_and_app_wasi_memory(
    tmp_path: Path,
) -> None:
    runtime_path = _wasm_from_wat(
        tmp_path,
        "molt_runtime",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 2 funcref))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (func (export "molt_set_wasm_table_base") (param i64))
          (type $runtime_init_indirect (func (result i64)))
          (func $runtime_table_entry (result i64)
            i64.const 0)
          (elem (i32.const 1) func $runtime_table_entry)
          (global $heap (mut i32) (i32.const 2048))
          (func (export "molt_scratch_alloc") (param $size i64) (result i64)
            (local $ptr i32)
            global.get $heap
            local.set $ptr
            global.get $heap
            local.get $size
            i32.wrap_i64
            i32.add
            global.set $heap
            local.get $ptr
            i64.extend_i32_u)
          (func (export "molt_scratch_free") (param i64 i64))
          (func (export "molt_string_from_bytes") (param $ptr i32) (param $len i64) (param $out i32) (result i32)
            local.get $len
            i64.const 2
            i64.ne
            if
              unreachable
            end
            local.get $ptr
            i32.load8_u
            i32.const 111
            i32.ne
            if
              unreachable
            end
            local.get $ptr
            i32.const 1
            i32.add
            i32.load8_u
            i32.const 107
            i32.ne
            if
              unreachable
            end
            local.get $out
            i64.const 1234
            i64.store
            i32.const 0)
          (func (export "molt_runtime_init") (result i64)
            i32.const 1
            call_indirect (type $runtime_init_indirect))
        )
        """,
    )
    app_path = _wasm_from_wat(
        tmp_path,
        "split_app_own_memory_wasi",
        """
        (module
          (import "env" "__indirect_function_table" (table 2 funcref))
          (import "molt_runtime" "runtime_init" (func $rt (result i64)))
          (import "molt_runtime" "molt_string_from_bytes"
            (func $string_from_bytes (param i32 i64 i32) (result i32)))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 1)
          (data (i32.const 16) "\\40\\00\\00\\00\\02\\00\\00\\00")
          (data (i32.const 64) "ok")
          (func (export "molt_main")
            call $rt
            drop
            i32.const 64
            i64.const 2
            i32.const 40
            call $string_from_bytes
            i32.const 0
            i32.ne
            if
              unreachable
            end
            i32.const 40
            i64.load
            i64.const 1234
            i64.ne
            if
              unreachable
            end
            i32.const 1
            i32.const 16
            i32.const 1
            i32.const 32
            call $fd_write
            drop)
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(app_path)],
        cwd=ROOT,
        env={
            **os.environ,
            "MOLT_RUNTIME_WASM": str(runtime_path),
            "MOLT_WASM_DIRECT_LINK": "1",
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_bridges_call_dispatch_argv_from_app_memory(
    tmp_path: Path,
) -> None:
    runtime_path = _wasm_from_wat(
        tmp_path,
        "molt_runtime_call_dispatch",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (global $heap (mut i32) (i32.const 2048))
          (func (export "molt_set_wasm_table_base") (param i64))
          (func (export "molt_scratch_alloc") (param $size i64) (result i64)
            (local $ptr i32)
            global.get $heap
            local.set $ptr
            global.get $heap
            local.get $size
            i32.wrap_i64
            i32.add
            global.set $heap
            local.get $ptr
            i64.extend_i32_u)
          (func (export "molt_scratch_free") (param i64 i64))
          (func (export "molt_runtime_init") (result i64)
            i64.const 0)
          (func (export "molt_call_func_dispatch")
            (param $func i64) (param $argv i64) (param $argc i64) (param $code i64)
            (result i64)
            local.get $func
            i64.const 999
            i64.ne
            if
              unreachable
            end
            local.get $argc
            i64.const 3
            i64.ne
            if
              unreachable
            end
            local.get $argv
            i32.wrap_i64
            i64.load
            i64.const 11
            i64.ne
            if
              unreachable
            end
            local.get $argv
            i32.wrap_i64
            i32.const 8
            i32.add
            i64.load
            i64.const 22
            i64.ne
            if
              unreachable
            end
            local.get $argv
            i32.wrap_i64
            i32.const 16
            i32.add
            i64.load
            i64.const 33
            i64.ne
            if
              unreachable
            end
            local.get $code
            i64.const 0
            i64.ne
            if
              unreachable
            end
            i64.const 777)
        )
        """,
    )
    app_path = _wasm_from_wat(
        tmp_path,
        "split_app_call_dispatch_own_memory",
        """
        (module
          (import "env" "__indirect_function_table" (table 1 funcref))
          (import "molt_runtime" "runtime_init" (func $rt (result i64)))
          (import "molt_runtime" "call_func_dispatch"
            (func $dispatch (param i64 i64 i64 i64) (result i64)))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 1)
          (data (i32.const 16) "\\40\\00\\00\\00\\02\\00\\00\\00")
          (data (i32.const 64) "ok")
          (func (export "molt_main")
            call $rt
            drop
            i32.const 96
            i64.const 11
            i64.store
            i32.const 104
            i64.const 22
            i64.store
            i32.const 112
            i64.const 33
            i64.store
            i64.const 999
            i64.const 96
            i64.const 3
            i64.const 0
            call $dispatch
            i64.const 777
            i64.ne
            if
              unreachable
            end
            i32.const 1
            i32.const 16
            i32.const 1
            i32.const 32
            call $fd_write
            drop)
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(app_path)],
        cwd=ROOT,
        env={
            **os.environ,
            "MOLT_RUNTIME_WASM": str(runtime_path),
            "MOLT_WASM_DIRECT_LINK": "1",
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_keeps_call_dispatch_argv_i64_with_shared_memory(
    tmp_path: Path,
) -> None:
    runtime_path = _wasm_from_wat(
        tmp_path,
        "molt_runtime_call_dispatch_shared_memory",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (func (export "molt_set_wasm_table_base") (param i64))
          (func (export "molt_runtime_init") (result i64)
            i64.const 0)
          (func (export "molt_call_func_dispatch")
            (param $func i64) (param $argv i64) (param $argc i64) (param $code i64)
            (result i64)
            local.get $func
            i64.const 999
            i64.ne
            if
              unreachable
            end
            local.get $argc
            i64.const 3
            i64.ne
            if
              unreachable
            end
            local.get $argv
            i32.wrap_i64
            i64.load
            i64.const 11
            i64.ne
            if
              unreachable
            end
            local.get $argv
            i32.wrap_i64
            i32.const 8
            i32.add
            i64.load
            i64.const 22
            i64.ne
            if
              unreachable
            end
            local.get $argv
            i32.wrap_i64
            i32.const 16
            i32.add
            i64.load
            i64.const 33
            i64.ne
            if
              unreachable
            end
            local.get $code
            i64.const 0
            i64.ne
            if
              unreachable
            end
            i64.const 777)
        )
        """,
    )
    app_path = _wasm_from_wat(
        tmp_path,
        "split_app_call_dispatch_shared_memory",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (import "molt_runtime" "runtime_init" (func $rt (result i64)))
          (import "molt_runtime" "call_func_dispatch"
            (func $dispatch (param i64 i64 i64 i64) (result i64)))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (data (i32.const 16) "\\40\\00\\00\\00\\02\\00\\00\\00")
          (data (i32.const 64) "ok")
          (func (export "molt_main")
            call $rt
            drop
            i32.const 96
            i64.const 11
            i64.store
            i32.const 104
            i64.const 22
            i64.store
            i32.const 112
            i64.const 33
            i64.store
            i64.const 999
            i64.const 96
            i64.const 3
            i64.const 0
            call $dispatch
            i64.const 777
            i64.ne
            if
              unreachable
            end
            i32.const 1
            i32.const 16
            i32.const 1
            i32.const 32
            call $fd_write
            drop)
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(app_path)],
        cwd=ROOT,
        env={
            **os.environ,
            "MOLT_RUNTIME_WASM": str(runtime_path),
            "MOLT_WASM_DIRECT_LINK": "1",
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_uses_runtime_export_signature_for_missing_wit(
    tmp_path: Path,
) -> None:
    runtime_path = _wasm_from_wat(
        tmp_path,
        "molt_runtime_export_signature_fallback",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (func (export "molt_set_wasm_table_base") (param i64))
          (func (export "molt_runtime_init") (result i64)
            i64.const 0)
          (func (export "molt_guarded_class_def")
            (param i64 i64 i64 i64 i64 i64 i64 i64) (result i64)
            local.get 1
            i64.const 1234
            i64.ne
            if
              unreachable
            end
            local.get 3
            i64.const 5678
            i64.ne
            if
              unreachable
            end
            i64.const 777)
        )
        """,
    )
    app_path = _wasm_from_wat(
        tmp_path,
        "split_app_runtime_export_signature_fallback",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (import "molt_runtime" "runtime_init" (func $rt (result i64)))
          (import "molt_runtime" "guarded_class_def"
            (func $class_def
              (param i64 i32 i64 i32 i64 i64 i64 i64) (result i64)))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (data (i32.const 16) "\\40\\00\\00\\00\\02\\00\\00\\00")
          (data (i32.const 64) "ok")
          (func (export "molt_main")
            call $rt
            drop
            i64.const 1
            i32.const 1234
            i64.const 2
            i32.const 5678
            i64.const 3
            i64.const 4
            i64.const 5
            i64.const 6
            call $class_def
            i64.const 777
            i64.ne
            if
              unreachable
            end
            i32.const 1
            i32.const 16
            i32.const 1
            i32.const 32
            call $fd_write
            drop)
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(app_path)],
        cwd=ROOT,
        env={
            **os.environ,
            "MOLT_RUNTIME_WASM": str(runtime_path),
            "MOLT_WASM_DIRECT_LINK": "1",
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_coerces_call_indirect_table_ref_i64_args(
    tmp_path: Path,
) -> None:
    runtime_path = _wasm_from_wat(
        tmp_path,
        "molt_runtime_call_indirect_i64",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 5 funcref))
          (import "env" "molt_call_indirect3"
            (func $call_indirect3 (param i64 i64 i64 i64) (result i64)))
          (func (export "molt_set_wasm_table_base") (param i64))
          (func (export "molt_runtime_init") (result i64)
            i64.const 4
            i64.const 11
            i64.const 22
            i64.const 33
            call $call_indirect3
            i64.const 66
            i64.ne
            if
              unreachable
            end
            i64.const 0)
        )
        """,
    )
    app_path = _wasm_from_wat(
        tmp_path,
        "split_app_call_indirect_table_ref_i64",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 5 funcref))
          (import "molt_runtime" "runtime_init" (func $rt (result i64)))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (data (i32.const 16) "\\40\\00\\00\\00\\02\\00\\00\\00")
          (data (i32.const 64) "ok")
          (func $target (param i64 i64 i64) (result i64)
            local.get 0
            local.get 1
            i64.add
            local.get 2
            i64.add)
          (elem (i32.const 4) func $target)
          (func (export "molt_main")
            call $rt
            drop
            i32.const 1
            i32.const 16
            i32.const 1
            i32.const 32
            call $fd_write
            drop)
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(app_path)],
        cwd=ROOT,
        env={
            **os.environ,
            "MOLT_RUNTIME_WASM": str(runtime_path),
            "MOLT_WASM_DIRECT_LINK": "1",
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_dispatches_reserved_runtime_trampoline(
    tmp_path: Path,
) -> None:
    reserved_callable_count = len(WASM_RESERVED_RUNTIME_CALLABLES)
    reserved_callable_index = 5
    default_trampoline_idx = (
        DEFAULT_WASM_APP_TABLE_BASE
        + WASM_RESERVED_RUNTIME_CALLABLE_BASE
        + reserved_callable_count
        + reserved_callable_index
    )
    runtime_path = _wasm_from_wat(
        tmp_path,
        "molt_runtime_reserved_runtime_trampoline",
        f"""
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (import "env" "molt_call_indirect3"
            (func $call_indirect3 (param i64 i64 i64 i64) (result i64)))
          (func (export "molt_set_wasm_table_base") (param i64))
          (func (export "molt_object_init_subclass") (param i64) (result i64)
            local.get 0
            i64.const 7
            i64.add)
          (func (export "molt_runtime_init") (result i64)
            i32.const 128
            i64.const 35
            i64.store
            ;; default app table base + reserved base + generated reserved count + index 5.
            i64.const {default_trampoline_idx}
            i64.const 0
            i64.const 128
            i64.const 1
            call $call_indirect3)
        )
        """,
    )
    app_path = _wasm_from_wat(
        tmp_path,
        "split_app_reserved_runtime_trampoline",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 4200 funcref))
          (import "molt_runtime" "runtime_init" (func $rt (result i64)))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (data (i32.const 16) "\\40\\00\\00\\00\\02\\00\\00\\00")
          (data (i32.const 64) "ok")
          (func $anchor (result i64) i64.const 0)
          (elem (i32.const 4096) func $anchor)
          (func (export "molt_main")
            call $rt
            i64.const 42
            i64.ne
            if
              unreachable
            end
            i32.const 1
            i32.const 16
            i32.const 1
            i32.const 32
            call $fd_write
            drop)
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(app_path)],
        cwd=ROOT,
        env={
            **os.environ,
            "MOLT_RUNTIME_WASM": str(runtime_path),
            "MOLT_WASM_DIRECT_LINK": "1",
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_dispatches_runtime_owned_reserved_trampoline(
    tmp_path: Path,
) -> None:
    reserved_callable_count = len(WASM_RESERVED_RUNTIME_CALLABLES)
    app_trampoline_idx = (
        TEST_SHARED_WASM_TABLE_BASE
        + WASM_RESERVED_RUNTIME_CALLABLE_BASE
        + reserved_callable_count
    )
    runtime_path = _wasm_from_wat(
        tmp_path,
        "molt_runtime_reserved_trampoline_owns_reserved_slot",
        f"""
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (import "env" "molt_call_indirect3"
            (func $call_indirect3 (param i64 i64 i64 i64) (result i64)))
          (func (export "molt_set_wasm_table_base") (param i64))
          (func (export "molt_type_call") (param i64) (result i64)
            local.get 0
            i64.const 44
            i64.ne
            if
              unreachable
            end
            i64.const 77)
          (func (export "molt_runtime_init") (result i64)
            i32.const 128
            i64.const 44
            i64.store
            ;; shared table base + reserved base + generated reserved count.
            i64.const {app_trampoline_idx}
            i64.const 0
            i64.const 128
            i64.const 1
            call $call_indirect3)
        )
        """,
    )
    app_path = _wasm_from_wat(
        tmp_path,
        "split_app_reserved_trampoline_trap_stub",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 4200 funcref))
          (import "molt_runtime" "runtime_init" (func $rt (result i64)))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (data (i32.const 16) "\\40\\00\\00\\00\\02\\00\\00\\00")
          (data (i32.const 64) "ok")
          (func $anchor (result i64) i64.const 0)
          (elem (i32.const 4096) func $anchor)
          (func (export "molt_main")
            call $rt
            i64.const 77
            i64.ne
            if
              unreachable
            end
            i32.const 1
            i32.const 16
            i32.const 1
            i32.const 32
            call $fd_write
            drop)
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(app_path)],
        cwd=ROOT,
        env={
            **os.environ,
            "MOLT_RUNTIME_WASM": str(runtime_path),
            "MOLT_WASM_DIRECT_LINK": "1",
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_runner_verifies_linker_published_callable_table() -> None:
    runner = (ROOT / "wasm" / "run_wasm.js").read_text(encoding="utf-8")

    assert "hasExportedTableRefs(outputInstance)" not in runner
    assert "molt_table_init" not in runner
    assert "restoreTablePrefix" not in runner
    assert "table.set(" not in runner
    assert "installTableRefs" not in runner
    assert (
        "verifyCallableTableEntries(outputCallableTable, table, 'output wasm');"
        in runner
    )


def _remap_default_app_runtime_shared_table_index(
    idx: int,
    *,
    shared_table_base: int,
    reserved_callable_count: int,
    installed_indices: tuple[int, ...] = (),
) -> int:
    """Invoke the JS ``remapDefaultAppRuntimeSharedTableIndex`` via node.

    ``installed_indices`` model raw table slots that already hold an installed
    funcref (the ``rawIndexHasInstalledEntry`` predicate returns true for them).
    """
    script = (
        "const bridge = require('./wasm/loader_bridge.js');"
        "const installed = new Set(JSON.parse(process.argv[2]));"
        "const out = bridge.remapDefaultAppRuntimeSharedTableIndex(Number(process.argv[1]), {"
        "  sharedTableBase: %d,"
        "  defaultAppTableBase: %d,"
        "  reservedRuntimeCallableBase: %d,"
        "  reservedRuntimeCallableCount: %d,"
        "  rawIndexHasInstalledEntry: (i) => installed.has(i),"
        "});"
        "process.stdout.write(String(out));"
    ) % (
        shared_table_base,
        DEFAULT_WASM_APP_TABLE_BASE,
        WASM_RESERVED_RUNTIME_CALLABLE_BASE,
        reserved_callable_count,
    )
    result = _run_wasm_test_process(
        ["node", "-e", script, str(idx), json.dumps(list(installed_indices))],
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    return int(result.stdout)


def test_remap_relocates_bare_default_base_reserved_reference() -> None:
    """A bare (unpopulated) default-base reserved reference is relocated to the
    live shared-table reserved region."""
    reserved_callable_count = len(WASM_RESERVED_RUNTIME_CALLABLES)
    # An index inside the default-base direct-reserved region with no installed slot.
    reserved_offset = 14
    default_idx = (
        DEFAULT_WASM_APP_TABLE_BASE
        + WASM_RESERVED_RUNTIME_CALLABLE_BASE
        + reserved_offset
    )
    remapped = _remap_default_app_runtime_shared_table_index(
        default_idx,
        shared_table_base=TEST_SHARED_WASM_TABLE_BASE,
        reserved_callable_count=reserved_callable_count,
        installed_indices=(),
    )
    assert (
        remapped
        == default_idx - DEFAULT_WASM_APP_TABLE_BASE + TEST_SHARED_WASM_TABLE_BASE
    )


def test_remap_preserves_installed_app_slot_in_default_region() -> None:
    """Regression: an app-local function pointer installed at a raw table index
    that happens to fall inside the default reserved region MUST be dispatched
    directly, not hijacked into a reserved runtime callable.

    This is the pact-witness capsule misdispatch: a real 4-arg app funcref at
    table index (default_app_base + reserved_base + 14) was remapped into the
    reserved region and dispatched to molt_types_capsule_new, raising a spurious
    ``TypeError: cannot create 'capsule' instances`` during numpy import.
    """
    reserved_callable_count = len(WASM_RESERVED_RUNTIME_CALLABLES)
    reserved_offset = 14
    installed_idx = (
        DEFAULT_WASM_APP_TABLE_BASE
        + WASM_RESERVED_RUNTIME_CALLABLE_BASE
        + reserved_offset
    )
    remapped = _remap_default_app_runtime_shared_table_index(
        installed_idx,
        shared_table_base=TEST_SHARED_WASM_TABLE_BASE,
        reserved_callable_count=reserved_callable_count,
        installed_indices=(installed_idx,),
    )
    # An occupied slot is a genuine indirect call: index is returned unchanged.
    assert remapped == installed_idx
