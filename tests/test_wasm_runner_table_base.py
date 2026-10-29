from __future__ import annotations

import functools
import json
import os
import shutil
from pathlib import Path

import pytest
from molt._wasm_abi_generated import (
    WASM_CALLABLE_TABLE_SECTION_NAME,
    WASM_CALLABLE_TABLE_SECTION_VERSION,
    WASM_CALLABLE_TABLE_VALUE_TYPE_FORMAT,
    WASM_RESERVED_RUNTIME_CALLABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLES,
)
from tests.wasm_linked_runner import _run_wasm_test_process
from tests.wasm_import_fixtures import build_wasm_tag_import_before_memory
from tests.wasm_execution_manifest import write_wasm_execution_manifest
from tools.wasm_link_format import _write_varuint

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_WASM_APP_TABLE_BASE = 256
TEST_SHARED_WASM_TABLE_BASE = 4096


def test_node_manifest_rejects_incompatible_receiver_before_module_read(
    tmp_path: Path,
) -> None:
    manifest_path = tmp_path / "manifest.json"
    script = (
        "const manifestPath = "
        + json.dumps(str(manifest_path))
        + ";\n"
        + r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {resolveWasmPaths} = require('./wasm/run_wasm.js');
const invalid = [undefined, null, 0, 1, 2, 3, 99, true, false, '4', 4.5];
const originalRead = fs.readFileSync;
let cases = 0;
for (const mode of ['linked', 'split-runtime']) {
  for (const version of invalid) {
    const descriptor = {path: 'must-not-read.wasm', size: 8, sha256: '0'.repeat(64)};
    const manifest = {version, mode, modules: mode === 'linked'
      ? {linked: descriptor} : {app: descriptor, runtime: descriptor}};
    fs.writeFileSync(manifestPath, JSON.stringify(manifest));
    let moduleReads = 0;
    fs.readFileSync = function(selected, ...args) {
      if (path.resolve(String(selected)) !== path.resolve(manifestPath)) {
        moduleReads++;
        throw new Error('module read attempted before receiver admission');
      }
      return originalRead.call(this, selected, ...args);
    };
    try {
      assert.throws(() => resolveWasmPaths({manifestPath}),
        /Molt deployment manifest version must be 4/);
      assert.equal(moduleReads, 0);
      cases++;
    } finally { fs.readFileSync = originalRead; }
  }
}
console.log(JSON.stringify({cases}));
"""
    )
    result = _run_wasm_test_process(
        ["node", "-e", script], role="tool", cwd=ROOT, env=os.environ, timeout=30
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {"cases": 22}


@pytest.mark.parametrize("capture", [False, True])
def test_direct_node_runner_passes_manifest_tail_to_wasi_guest(
    tmp_path: Path, capture: bool
) -> None:
    wasm_tools = shutil.which("wasm-tools")
    if wasm_tools is None:
        pytest.skip("wasm-tools is required for the WASI argv fixture")
    wat_path = tmp_path / "argv_guest.wat"
    wasm_path = tmp_path / "argv_guest.wasm"
    wat_path.write_text(
        """
        (module
          (import "wasi_snapshot_preview1" "args_sizes_get"
            (func $sizes (param i32 i32) (result i32)))
          (import "wasi_snapshot_preview1" "args_get"
            (func $args (param i32 i32) (result i32)))
          (memory (export "molt_memory") 1)
          (table (export "molt_table") 1 funcref)
          (func $expect_byte (param $ptr i32) (param $offset i32) (param $value i32)
            local.get $ptr
            local.get $offset
            i32.add
            i32.load8_u
            local.get $value
            i32.ne
            if unreachable end)
          (func (export "molt_runtime_execution_enter") (result i64) i64.const 1)
          (func (export "molt_runtime_execution_leave") (param i64))
          (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
          (func (export "molt_exception_pending") (result i64) i64.const 0)
          (func (export "molt_main_exit_status") (result i64) i64.const 0)
          (func (export "molt_main")
            i32.const 0
            i32.const 4
            call $sizes
            if unreachable end
            i32.const 0
            i32.load
            i32.const 3
            i32.ne
            if unreachable end
            i32.const 4
            i32.load
            i32.const 20
            i32.ne
            if unreachable end
            i32.const 16
            i32.const 64
            call $args
            if unreachable end
            i32.const 16
            i32.load
            i32.const 0
            i32.const 109 ;; molt
            call $expect_byte
            i32.const 20
            i32.load
            i32.const 0
            i32.const 45 ;; --sample
            call $expect_byte
            i32.const 20
            i32.load
            i32.const 7
            i32.const 101
            call $expect_byte
            i32.const 24
            i32.load
            i32.const 0
            i32.const 118 ;; value
            call $expect_byte
            i32.const 24
            i32.load
            i32.const 4
            i32.const 101
            call $expect_byte)
        )
        """,
        encoding="utf-8",
    )
    parsed = _run_wasm_test_process(
        [wasm_tools, "parse", str(wat_path), "-o", str(wasm_path)],
        role="tool",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert parsed.returncode == 0, parsed.stderr
    _attest_empty_callable_table(wasm_path)
    manifest = _linked_manifest(wasm_path)
    command = ["node", "wasm/run_wasm.js"]
    if capture:
        capture_path = tmp_path / "guest.stderr"
        capture_path.touch()
        command.extend(["--guest-stderr", str(capture_path), "--"])
    command.append(str(manifest))
    env = dict(os.environ)
    env.pop("NODE_NO_WARNINGS", None)

    omitted = _run_wasm_test_process(
        command, role="guest-host", cwd=ROOT, env=env, timeout=30
    )
    passed = _run_wasm_test_process(
        [*command, "--sample", "value"],
        role="guest-host",
        cwd=ROOT,
        env=env,
        timeout=30,
    )

    assert omitted.returncode != 0
    assert passed.returncode == 0, passed.stderr
    if capture:
        assert capture_path.read_bytes() == b""


def _attest_empty_callable_table(wasm_path: Path) -> None:
    # This fixture has no callable-table entries. Publish the generated ABI's
    # empty attestation, without invoking the Rust publishing build lane.
    name = WASM_CALLABLE_TABLE_SECTION_NAME.encode("utf-8")
    attestation = b"".join(
        _write_varuint(value)
        for value in (
            WASM_CALLABLE_TABLE_SECTION_VERSION,
            WASM_CALLABLE_TABLE_VALUE_TYPE_FORMAT,
            0,
            0,
        )
    )
    payload = _write_varuint(len(name)) + name + attestation
    wasm_path.write_bytes(
        wasm_path.read_bytes() + b"\x00" + _write_varuint(len(payload)) + payload
    )


def _linked_manifest(wasm_path: Path) -> Path:
    return write_wasm_execution_manifest(wasm_path.parent, linked=wasm_path)


def _split_manifest(app_path: Path, runtime_path: Path) -> Path:
    return write_wasm_execution_manifest(
        app_path.parent, app=app_path, runtime=runtime_path
    )


@functools.cache
def _wasm_facts_tool() -> Path:
    """The facts tool `molt build --target wasm` admits for linked output.

    Fixtures with callable entries publish their link facts with that tool;
    known-empty synthetic tables use the generated ABI's empty attestation.
    The codegen backend exposes no facts mode since the tool was separated
    (docs/spec/areas/compiler/0931_LINKER_OPTIMIZATION_CONTRACT.md).
    """
    from molt.cli.wasm_facts_tool import ensure_wasm_facts_tool

    return ensure_wasm_facts_tool(ROOT, env=os.environ)


def _wasm_from_wat(
    tmp_path: Path,
    name: str,
    wat: str,
    *,
    callable_layout: tuple[int, int, int, int] | None = None,
    empty_callable_table: bool = False,
) -> Path:
    wasm_tools = shutil.which("wasm-tools")
    if wasm_tools is None:
        pytest.skip("wasm-tools is required for runner table-base parser tests")
    wat_path = tmp_path / f"{name}.wat"
    wasm_path = tmp_path / f"{name}.wasm"
    wat_path.write_text(wat, encoding="utf-8")
    result = _run_wasm_test_process(
        [wasm_tools, "parse", str(wat_path), "-o", str(wasm_path)],
        role="tool",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    if empty_callable_table:
        assert callable_layout is None
        _attest_empty_callable_table(wasm_path)
        return wasm_path
    published = wasm_path.with_suffix(".published.wasm")
    publish_command = [
        str(_wasm_facts_tool()),
        "--publish-wasm-link-facts",
        "--output",
        str(published),
    ]
    if callable_layout is not None:
        publish_command.extend(
            [
                "--callable-table-layout",
                ",".join(str(value) for value in callable_layout),
                "--callable-table-role",
                "monolithic",
            ]
        )
    # The facts tool reads the module from stdin.
    result = _run_wasm_test_process(
        publish_command,
        role="tool",
        cwd=ROOT,
        env=os.environ,
        timeout=300,
        input=wasm_path.read_bytes(),
        text=False,
    )
    assert result.returncode == 0, result.stderr
    published.replace(wasm_path)
    return wasm_path


def _execution_wasm_from_wat(tmp_path: Path, name: str, wat: str) -> Path:
    source = wat.rstrip()
    if not source.endswith(")"):
        raise ValueError("synthetic executable WAT must end with its module delimiter")
    execution_boundary = """
      (func (export "molt_runtime_execution_enter") (result i64)
        i64.const 1)
      (func (export "molt_runtime_execution_leave") (param i64))
      (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
      (func (export "molt_exception_pending") (result i64) i64.const 0)
      (func (export "molt_main_exit_status") (result i64) i64.const 0)
    """
    return _wasm_from_wat(
        tmp_path,
        name,
        source[:-1] + execution_boundary + ")",
    )


def _stderr_guest_manifest(
    tmp_path: Path, layout: str, *, trap: bool, close_app_stderr: bool = False
) -> Path:
    guest_bytes = b"ExperimentalWarning: guest\r\n(node:7) guest\n\xfftail"
    escaped = "".join(f"\\{value:02x}" for value in guest_bytes)
    wasi_import = """
      (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
    """
    if close_app_stderr:
        wasi_import += '(import "wasi_snapshot_preview1" "fd_close" (func $close (param i32) (result i32)))'
    writer = """
      (func $emit (param $fd i32) (param $ptr i32) (param $len i32)
        i32.const 0 local.get $ptr i32.store
        i32.const 4 local.get $len i32.store
        local.get $fd i32.const 0 i32.const 1 i32.const 8 call $write
        if unreachable end)
    """
    lifecycle = """
      (func (export "molt_runtime_execution_enter") (result i64) i64.const 1)
      (func (export "molt_runtime_execution_leave") (param i64))
      (func (export "molt_exception_pending") (result i64) i64.const 0)
      (func (export "molt_main_exit_status") (result i64) i64.const 0)
      (func (export "molt_runtime_shutdown") (result i64)
        i32.const 2 i32.const 256 i32.const 9 call $emit i64.const 1)
      (data (i32.const 256) "|shutdown")
    """
    main = f"""
      (data (i32.const 128) "{escaped}")
      (data (i32.const 224) "stdout")
      (func (export "molt_main")
        {"call $init drop" if layout == "split" else ""}
        i32.const 1 i32.const 224 i32.const 6 call $emit
        i32.const 2 i32.const 128 i32.const {len(guest_bytes)} call $emit
        {"i32.const 2 call $close if unreachable end" if close_app_stderr else ""}
        {"unreachable" if trap else ""})
    """
    if layout == "linked":
        linked = _wasm_from_wat(
            tmp_path,
            "stderr_linked",
            f"""(module {wasi_import}
              (memory (export "memory") 1)
              (table (export "molt_table") 1 funcref)
              {writer} {lifecycle} {main})""",
            empty_callable_table=True,
        )
        return _linked_manifest(linked)
    runtime = _wasm_from_wat(
        tmp_path,
        "stderr_runtime",
        f"""(module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          {wasi_import}
          (export "memory" (memory 0))
          (func (export "molt_set_wasm_table_base") (param i64))
          {writer} {lifecycle}
          (data (i32.const 320) "runtime|")
          (func (export "molt_runtime_init") (result i64)
            i32.const 2 i32.const 320 i32.const 8 call $emit i64.const 1))""",
        empty_callable_table=True,
    )
    app = _wasm_from_wat(
        tmp_path,
        "stderr_app",
        f"""(module
          (import "env" "__indirect_function_table" (table 1 funcref))
          (import "molt_runtime" "runtime_init" (func $init (result i64)))
          {wasi_import}
          (memory (export "memory") 1)
          {writer} {main})""",
        empty_callable_table=True,
    )
    return _split_manifest(app, runtime)


@pytest.mark.parametrize("layout", ["linked", "split"])
@pytest.mark.parametrize("trap", [False, True])
def test_direct_node_runner_keeps_exact_guest_stderr_separate_from_host(
    tmp_path: Path, layout: str, trap: bool
) -> None:
    manifest = _stderr_guest_manifest(tmp_path, layout, trap=trap)
    capture = tmp_path / "guest.stderr"
    capture.touch()
    env = {**os.environ, "MOLT_WASM_TRACE_RUN": "1"}
    env.pop("NODE_NO_WARNINGS", None)
    result = _run_wasm_test_process(
        [
            "node",
            "wasm/run_wasm.js",
            "--guest-stderr",
            str(capture),
            "--",
            str(manifest),
        ],
        role="guest-host",
        cwd=ROOT,
        env=env,
        timeout=30,
        text=False,
    )
    assert result.returncode == (1 if trap else 0), result.stderr
    assert result.stdout == b"stdout"
    prefix = b"runtime|" if layout == "split" else b""
    guest_bytes = b"ExperimentalWarning: guest\r\n(node:7) guest\n\xfftail"
    assert capture.read_bytes() == prefix + guest_bytes + b"|shutdown"
    assert b"[molt wasm] runner=" in result.stderr
    assert guest_bytes not in result.stderr
    assert (
        b"unreachable" in result.stderr if trap else b"unreachable" not in result.stderr
    )


def test_direct_split_runner_app_stderr_close_cannot_retarget_runtime_shutdown(
    tmp_path: Path,
) -> None:
    manifest = _stderr_guest_manifest(
        tmp_path, "split", trap=False, close_app_stderr=True
    )
    capture = tmp_path / "guest.stderr"
    capture.touch()
    sentinel = tmp_path / "unrelated-sentinel"
    sentinel.write_bytes(b"UNCHANGED_SENTINEL")
    reuse_record = tmp_path / "reuse.json"
    injector = tmp_path / "reuse_closed_fd.cjs"
    injector.write_text(
        "const fs = require('fs');\n"
        "const wasi = require('node:wasi');\n"
        "const Base = wasi.WASI;\n"
        "let sentinelFd = null;\n"
        "wasi.WASI = class extends Base {\n"
        "  constructor(options) {\n"
        "    super(options);\n"
        "    const close = this.wasiImport.fd_close;\n"
        "    this.wasiImport.fd_close = (fd) => {\n"
        "      const result = close(fd);\n"
        "      if (fd === 2 && result === 0) {\n"
        f"        sentinelFd = fs.openSync({json.dumps(str(sentinel))}, 'r+');\n"
        "        if (sentinelFd !== options.stderr) throw new Error('closed host FD was not reused');\n"
        f"        fs.writeFileSync({json.dumps(str(reuse_record))}, JSON.stringify({{ closed: options.stderr, reused: sentinelFd }}));\n"
        "      }\n"
        "      return result;\n"
        "    };\n"
        "  }\n"
        "};\n"
        "process.on('exit', () => {\n"
        "  if (sentinelFd !== null) fs.closeSync(sentinelFd);\n"
        "});\n",
        encoding="utf-8",
    )
    result = _run_wasm_test_process(
        [
            "node",
            "--require",
            str(injector),
            "wasm/run_wasm.js",
            "--guest-stderr",
            str(capture),
            "--",
            str(manifest),
        ],
        role="guest-host",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
        text=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == b"stdout"
    record = json.loads(reuse_record.read_text(encoding="utf-8"))
    assert record["closed"] == record["reused"]
    assert capture.read_bytes() == (
        b"runtime|ExperimentalWarning: guest\r\n(node:7) guest\n\xfftail|shutdown"
    )
    assert sentinel.read_bytes() == b"UNCHANGED_SENTINEL"


@pytest.mark.parametrize("mutation", ["close", "move", "replace"])
def test_direct_node_runner_retires_capture_after_guest_fd_ownership_changes(
    tmp_path: Path, mutation: str
) -> None:
    operation = {
        "close": "i32.const 2 call $close if unreachable end",
        "move": "i32.const 2 i32.const 1 call $renumber if unreachable end",
        "replace": "i32.const 1 i32.const 2 call $renumber if unreachable end",
    }[mutation]
    after = {
        "close": "",
        "move": "i32.const 1 call $write_tail",
        "replace": "i32.const 2 call $write_tail",
    }[mutation]
    module = _execution_wasm_from_wat(
        tmp_path,
        "capture_fd_ownership",
        f"""(module
          (import "wasi_snapshot_preview1" "fd_write"
            (func $write (param i32 i32 i32 i32) (result i32)))
          (import "wasi_snapshot_preview1" "fd_close"
            (func $close (param i32) (result i32)))
          (import "wasi_snapshot_preview1" "fd_renumber"
            (func $renumber (param i32 i32) (result i32)))
          (memory (export "memory") 1)
          (table (export "molt_table") 1 funcref)
          (data (i32.const 64) "tail")
          (func $write_tail (param $fd i32)
            i32.const 0 i32.const 64 i32.store
            i32.const 4 i32.const 4 i32.store
            local.get $fd i32.const 0 i32.const 1 i32.const 8 call $write
            if unreachable end)
          (func (export "molt_main")
            i32.const 2 call $write_tail
            {operation}
            {after}))""",
    )
    capture = tmp_path / "guest.stderr"
    capture.touch()
    result = _run_wasm_test_process(
        [
            "node",
            "wasm/run_wasm.js",
            "--guest-stderr",
            str(capture),
            "--",
            str(_linked_manifest(module)),
        ],
        role="guest-host",
        cwd=ROOT,
        env={**os.environ, "MOLT_WASM_TRACE_RUN": "1"},
        timeout=30,
        text=False,
    )
    assert result.returncode == 0, result.stderr
    assert capture.read_bytes() == (b"tailtail" if mutation == "move" else b"tail")
    assert result.stdout == (b"tail" if mutation == "replace" else b"")
    assert b"[molt wasm] runner completed" in result.stderr


def test_direct_node_runner_capture_admission_preserves_existing_bytes(
    tmp_path: Path,
) -> None:
    capture = tmp_path / "guest.stderr"
    capture.write_bytes(b"preserve")
    result = _run_wasm_test_process(
        [
            "node",
            "wasm/run_wasm.js",
            "--guest-stderr",
            str(capture),
            "--",
            "absent.json",
        ],
        role="guest-host",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 1
    assert "existing empty regular file" in result.stderr
    assert capture.read_bytes() == b"preserve"


@pytest.mark.parametrize("kind", ["symlink", "directory", "fifo"])
def test_direct_node_runner_rejects_nonregular_capture_before_open(
    tmp_path: Path, kind: str
) -> None:
    capture = tmp_path / "guest.stderr"
    original = tmp_path / "original"
    original.touch()
    if kind == "symlink":
        try:
            capture.symlink_to(original)
        except OSError as exc:
            pytest.skip(f"symlink creation is unavailable: {exc}")
    elif kind == "directory":
        capture.mkdir()
    else:
        if not hasattr(os, "mkfifo"):
            pytest.skip("FIFO paths are unavailable on this host")
        os.mkfifo(capture)
    result = _run_wasm_test_process(
        [
            "node",
            "wasm/run_wasm.js",
            "--guest-stderr",
            str(capture),
            "--",
            "absent.json",
        ],
        role="guest-host",
        cwd=ROOT,
        env=os.environ,
        timeout=10,
    )
    assert result.returncode == 1
    assert "existing empty regular file" in result.stderr
    assert original.read_bytes() == b""
    if kind == "symlink":
        assert capture.is_symlink()


@pytest.mark.parametrize("replace_at", ["before_open", "after_open"])
def test_direct_node_runner_rejects_capture_inode_substitution(
    tmp_path: Path, replace_at: str
) -> None:
    capture = tmp_path / "guest.stderr"
    capture.touch()
    held_original = tmp_path / "held-original"
    replacement = tmp_path / "replacement"
    replacement.touch()
    injector = tmp_path / "replace_capture.cjs"
    injector.write_text(
        "const fs = require('fs');\n"
        "const originalOpen = fs.openSync;\n"
        f"const capture = {json.dumps(str(capture))};\n"
        "fs.openSync = function(candidate, ...args) {\n"
        "  if (candidate !== capture) return originalOpen.call(this, candidate, ...args);\n"
        "  const substitute = () => {\n"
        f"    fs.renameSync(capture, {json.dumps(str(held_original))});\n"
        f"    fs.renameSync({json.dumps(str(replacement))}, capture);\n"
        "  };\n"
        + (
            "  substitute();\n  return originalOpen.call(this, candidate, ...args);\n"
            if replace_at == "before_open"
            else "  const fd = originalOpen.call(this, candidate, ...args);\n  substitute();\n  return fd;\n"
        )
        + "};\n",
        encoding="utf-8",
    )
    result = _run_wasm_test_process(
        [
            "node",
            "--require",
            str(injector),
            "wasm/run_wasm.js",
            "--guest-stderr",
            str(capture),
            "--",
            "absent.json",
        ],
        role="guest-host",
        cwd=ROOT,
        env=os.environ,
        timeout=10,
    )
    assert result.returncode == 1
    assert "capture path identity changed during admission" in result.stderr
    assert held_original.read_bytes() == b""
    assert capture.read_bytes() == b""


@pytest.mark.parametrize("tail", [[], ["capture"], ["capture", "manifest.json"]])
def test_direct_node_runner_rejects_ambiguous_capture_prefix(tail: list[str]) -> None:
    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", "--guest-stderr", *tail],
        role="guest-host",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 1
    assert "usage: run_wasm.js --guest-stderr PATH -- MANIFEST" in result.stderr


def _extract_table_base(wasm_path: Path) -> int | None:
    script = (
        "const fs = require('fs');"
        "const runner = require('./wasm/run_wasm.js');"
        "const value = runner.extractWasmTableBase(fs.readFileSync(process.argv[1]));"
        "process.stdout.write(JSON.stringify(value));"
    )
    result = _run_wasm_test_process(
        ["node", "-e", script, str(wasm_path)],
        role="tool",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def test_host_socket_errors_reach_the_guest_in_the_declared_domain() -> None:
    script = """
const runner = require('./wasm/run_wasm.js');
const util = require('node:util');
const errno = require('./wasm/synthetic_errno_generated.js').errno;
const nativeWouldBlock = [...util.getSystemErrorMap()].find(([, detail]) => detail[0] === 'EAGAIN')[0];
process.stdout.write(JSON.stringify({
  pending: runner.mapSocketError({code:'EINPROGRESS',errno:-1}),
  refused: runner.mapSocketError({code:'ECONNREFUSED'}),
  wouldBlock: runner.mapSocketError({errno:nativeWouldBlock}),
  unknown: runner.mapSocketError({errno:-2147483648}),
  resolver: runner.mapResolverFailure({code:'ENOTFOUND'}),
  foreignResolver: runner.mapSocketError({code:'EAI_NONAME',errno:-9}),
  resolverCollision: runner.mapResolverFailure({code:'EAI_SERVICE'}),
  systemCollision: runner.mapResolverFailure({code:'EBADF'}),
  numericSystem: runner.mapResolverFailure({errno: [...util.getSystemErrorMap()].find(([, detail]) => detail[0] === 'EBADF')[0]}),
  alias: errno.EWOULDBLOCK,
  explicitUnknown: runner.mapSocketError({code:'EFAKE',errno:nativeWouldBlock}),
  unknownResolverBounded: (() => { try { runner.mapResolverFailure({code: 'X'.repeat(10000)}); return false; } catch (error) { return error.constructor.name === 'ResolverBoundaryError' && error.message.length < 200; } })(),
  ordinaryNoDataRefused: (() => { try { runner.mapResolverFailure({code:'ENODATA'}); return false; } catch (error) { return error.constructor.name === 'ResolverBoundaryError'; } })(),
}));
"""
    result = _run_wasm_test_process(
        ["node", "-e", script], role="tool", cwd=ROOT, env=os.environ, timeout=30
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {
        "pending": 115,
        "refused": 111,
        "wouldBlock": 11,
        "unknown": 5,
        "resolver": 8,
        "foreignResolver": 5,
        "resolverCollision": 9,
        "systemCollision": -9,
        "numericSystem": -9,
        "alias": 11,
        "explicitUnknown": 5,
        "unknownResolverBounded": True,
        "ordinaryNoDataRefused": True,
    }


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
        role="tool",
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


def test_extract_wasm_table_base_uses_published_fixed_runtime_prefix(
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
        callable_layout=(1, 1, 4096, 1),
    )

    assert _extract_table_base(wasm_path) == 1


def test_extract_wasm_table_base_uses_published_layout_after_segment_coalescing(
    tmp_path: Path,
) -> None:
    wasm_path = _wasm_from_wat(
        tmp_path,
        "coalesced_runtime_and_app_segment",
        """
        (module
          (table 4 funcref)
          (func $runtime)
          (func $app)
          (elem (i32.const 1) func $runtime $app)
        )
        """,
        callable_layout=(1, 1, 2, 1),
    )

    assert _extract_table_base(wasm_path) == 1


def test_extract_wasm_table_base_uses_app_base_without_fixed_runtime_prefix(
    tmp_path: Path,
) -> None:
    wasm_path = _wasm_from_wat(
        tmp_path,
        "app_only_callable_segment",
        """
        (module
          (table 9 funcref)
          (func $app)
          (elem (i32.const 8) func $app)
        )
        """,
        callable_layout=(0, 0, 8, 1),
    )

    assert _extract_table_base(wasm_path) == 8


def test_linked_runner_uses_env_table_base_without_calling_setter(
    tmp_path: Path,
) -> None:
    wasm_path = _execution_wasm_from_wat(
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
        ["node", "wasm/run_wasm.js", str(_linked_manifest(wasm_path))],
        role="guest-host",
        cwd=ROOT,
        env={**os.environ, "NODE_NO_WARNINGS": "1"},
        timeout=30,
    )
    assert result.returncode == 0, result.stderr


def test_linked_runner_uses_only_main_startup_authority(
    tmp_path: Path,
) -> None:
    wasm_path = _execution_wasm_from_wat(
        tmp_path,
        "linked_main_owns_startup",
        """
        (module
          (memory (export "molt_memory") 1)
          (func (export "molt_host_init") unreachable)
          (func (export "molt_isolate_bootstrap") (result i64) unreachable)
          (func (export "molt_main"))
        )
        """,
    )

    result = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(_linked_manifest(wasm_path))],
        role="guest-host",
        cwd=ROOT,
        env={**os.environ, "NODE_NO_WARNINGS": "1"},
        timeout=30,
    )
    assert result.returncode == 0, result.stderr


def test_direct_split_runner_accepts_runtime_memory_and_app_wasi_memory(
    tmp_path: Path,
) -> None:
    runtime_path = _execution_wasm_from_wat(
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
        ["node", "wasm/run_wasm.js", str(_split_manifest(app_path, runtime_path))],
        role="guest-host",
        cwd=ROOT,
        env={
            **os.environ,
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_bridges_call_dispatch_argv_from_app_memory(
    tmp_path: Path,
) -> None:
    runtime_path = _execution_wasm_from_wat(
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
        ["node", "wasm/run_wasm.js", str(_split_manifest(app_path, runtime_path))],
        role="guest-host",
        cwd=ROOT,
        env={
            **os.environ,
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_keeps_call_dispatch_argv_i64_with_shared_memory(
    tmp_path: Path,
) -> None:
    runtime_path = _execution_wasm_from_wat(
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
        ["node", "wasm/run_wasm.js", str(_split_manifest(app_path, runtime_path))],
        role="guest-host",
        cwd=ROOT,
        env={
            **os.environ,
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_uses_runtime_export_signature_for_missing_wit(
    tmp_path: Path,
) -> None:
    runtime_path = _execution_wasm_from_wat(
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
        ["node", "wasm/run_wasm.js", str(_split_manifest(app_path, runtime_path))],
        role="guest-host",
        cwd=ROOT,
        env={
            **os.environ,
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_passes_pointer_imports_with_their_exact_types(
    tmp_path: Path,
) -> None:
    # The runner converts each runtime import by the runtime export's own
    # type. These imports carry a pointer as an i64, so a u32 reading would
    # turn the BigInt into a Number that an i64 parameter rejects.
    runtime_path = _execution_wasm_from_wat(
        tmp_path,
        "molt_runtime_pointer_imports",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (func (export "molt_set_wasm_table_base") (param i64))
          (func (export "molt_runtime_init") (result i64)
            i64.const 0)
          (func (export "molt_closure_load") (param i64 i64) (result i64)
            local.get 0
            i64.const 4096
            i64.ne
            if
              unreachable
            end
            local.get 0
            local.get 1
            i64.add)
          (func (export "molt_object_set_class") (param i64 i64) (result i64)
            local.get 0
            local.get 1
            i64.add)
        )
        """,
    )
    app_path = _wasm_from_wat(
        tmp_path,
        "split_app_pointer_imports",
        """
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (import "molt_runtime" "runtime_init" (func $rt (result i64)))
          (import "molt_runtime" "closure_load"
            (func $closure_load (param i64 i64) (result i64)))
          (import "molt_runtime" "object_set_class"
            (func $set_class (param i64 i64) (result i64)))
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (data (i32.const 16) "\\40\\00\\00\\00\\02\\00\\00\\00")
          (data (i32.const 64) "ok")
          (func (export "molt_main")
            call $rt
            drop
            i64.const 4096
            i64.const 8
            call $closure_load
            i64.const 4104
            i64.ne
            if
              unreachable
            end
            i64.const 4096
            i64.const 32
            call $set_class
            i64.const 4128
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
        ["node", "wasm/run_wasm.js", str(_split_manifest(app_path, runtime_path))],
        role="guest-host",
        cwd=ROOT,
        env={
            **os.environ,
            "NODE_NO_WARNINGS": "1",
        },
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "ok"


def test_direct_split_runner_coerces_call_indirect_table_ref_i64_args(
    tmp_path: Path,
) -> None:
    runtime_path = _execution_wasm_from_wat(
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
        ["node", "wasm/run_wasm.js", str(_split_manifest(app_path, runtime_path))],
        role="guest-host",
        cwd=ROOT,
        env={
            **os.environ,
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
    runtime_path = _execution_wasm_from_wat(
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
        ["node", "wasm/run_wasm.js", str(_split_manifest(app_path, runtime_path))],
        role="guest-host",
        cwd=ROOT,
        env={
            **os.environ,
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
    runtime_path = _execution_wasm_from_wat(
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
        ["node", "wasm/run_wasm.js", str(_split_manifest(app_path, runtime_path))],
        role="guest-host",
        cwd=ROOT,
        env={
            **os.environ,
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
        role="tool",
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


@pytest.mark.parametrize("trap", [False, True])
def test_cli_json_wasm_keeps_guest_bytes_and_host_diagnostics(
    tmp_path, monkeypatch, capsys, trap
):
    from molt.cli import script_commands
    from molt.process_environment import ProcessEnvironment
    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from molt.node_runtime import resolve_node_runtime

    manifest = _stderr_guest_manifest(tmp_path, "linked", trap=trap)
    env = {**os.environ, "MOLT_WASM_TRACE_RUN": "1"}
    env.pop("NODE_NO_WARNINGS", None)
    env.pop("NODE_OPTIONS", None)
    node = resolve_node_runtime(source_root=ROOT, environment=env)
    owners = []

    def allocate(**kwargs):
        owner = OwnedTemporaryDirectory(dir=tmp_path, **kwargs)
        owners.append(owner)
        return owner

    monkeypatch.setattr(script_commands, "OwnedTemporaryDirectory", allocate)
    rc = script_commands._run_program(
        [str(node.path), str(ROOT / "wasm/run_wasm.js"), str(manifest)],
        env=ProcessEnvironment(env),
        cwd=ROOT,
        json_output=True,
        verbose=False,
        timing=True,
        build_duration_s=0.0,
        start_receipt=None,
        memory_guard_prefix="MOLT_CROSS",
        wasm_guest_stderr=True,
    )
    payload = json.loads(capsys.readouterr().out)
    assert rc == (1 if trap else 0)
    assert payload["status"] == ("error" if trap else "ok")
    data = payload["data"]
    assert data["returncode"] == rc and data["stdout"] == "stdout"
    expected = b"ExperimentalWarning: guest\r\n(node:7) guest\n\xfftail|shutdown"
    assert data["stderr"].encode("utf-8", "surrogateescape") == expected
    assert "[molt wasm] runner=" in data["host_stderr"]
    assert "ExperimentalWarning" in data["host_stderr"]
    assert "ExperimentalWarning: guest" not in data["host_stderr"]
    assert ("unreachable" in data["host_stderr"]) is trap
    assert all(not Path(owner.name).exists() for owner in owners)


def test_resolver_worker_transport_preserves_domains_and_terminal_refusal(
    tmp_path: Path,
) -> None:
    # The real worker inherits this external DNS producer. No production test op,
    # special error status, or duplicate worker implementation is substituted.
    preload = tmp_path / "dns_producer.cjs"
    preload.write_text(
        """
const dns = require('node:dns');
dns.promises.lookup = async host => {
  const identities = { 'resolver.test': 'EAI_SERVICE', 'system.test': 'EBADF',
    'opaque.test': 'EAI_SYSTEM', 'unknown.test': 'EAI_VENDOR' };
  if (identities[host]) { const error = new Error('external DNS failure'); error.code = identities[host]; throw error; }
  return [{ address: '127.0.0.1', family: 4 }];
};
""",
        encoding="utf-8",
    )
    script = r"""
const fs = require('node:fs');
const Module = require('node:module');
const path = require('node:path');
const filename = path.resolve('wasm/run_wasm.js');
const loaded = new Module(filename, module);
loaded.filename = filename;
loaded.paths = Module._nodeModulePaths(path.dirname(filename));
loaded._compile(fs.readFileSync(filename, 'utf8') +
  '\nmodule.exports.transport = { socketHostGetaddrinfo, setWasmMemory, close: () => socketWorkerClient.close() };', filename);
const transport = loaded.exports.transport;
(async () => {
  // Import and re-export the real i32 host boundary; no compiler fixture or
  // alternate resolver implementation supplies a result.
  const text = value => [...Buffer.from(value)];
  const name = value => [text(value).length, ...text(value)];
  const section = (id, body) => [id, body.length, ...body];
  const bytes = [0,97,115,109,1,0,0,0,
    ...section(1, [1, 0x60, 11, ...Array(11).fill(0x7f), 1, 0x7f]),
    ...section(2, [2, ...name('env'), ...name('molt_socket_getaddrinfo_host'), 0, 0,
      ...name('env'), ...name('memory'), 2, 0, 1]),
    ...section(7, [2, ...name('lookup'), 0, 0, ...name('memory'), 2, 0])];
  const memory = new WebAssembly.Memory({initial: 1});
  transport.setWasmMemory(memory);
  const {instance} = await WebAssembly.instantiate(new Uint8Array(bytes), {
    env: {memory, molt_socket_getaddrinfo_host: transport.socketHostGetaddrinfo},
  });
  const statuses = [];
  const lengthControls = [];
  let requiredLength;
  try {
    for (const host of ['ok.test', 'resolver.test', 'system.test', 'opaque.test', 'unknown.test', 'ok.test']) {
      try {
        new Uint8Array(memory.buffer).set(Buffer.from(host), 64);
        new Uint8Array(memory.buffer).set(Buffer.from('80'), 512);
        statuses.push(instance.exports.lookup(64, Buffer.byteLength(host), 512, 2,
          2, 1, 6, 0, 1024, host === 'opaque.test' ? 0 : 1024, 2048));
      } catch (error) {
        statuses.push({type: error.constructor.name, diagnostic: error.message});
      }
    }
    for (const capacity of [0, 1024]) {
      lengthControls.push(instance.exports.lookup(64, 7, 512, 2, 2, 1, 6, 0, 1024, capacity, 65535));
    }
    lengthControls.push(instance.exports.lookup(64, 7, 512, 2, 2, 1, 6, 0, 1024, 0, 2048));
    requiredLength = new DataView(memory.buffer).getUint32(2048, true);
  } finally { await transport.close(); }
  console.log(JSON.stringify({statuses, lengthControls, requiredLength}));
})().catch(error => { console.error(error); process.exitCode = 1; });
"""
    result = _run_wasm_test_process(
        ["node", "--require", str(preload), "-e", script],
        role="guest-host",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    statuses = payload["statuses"]
    assert payload["lengthControls"] == [-14, -14, -12]
    assert payload["requiredLength"] > 0
    assert statuses[:3] == [0, 9, -9]
    assert (
        statuses[-1] == 0
    )  # A refusal wakes its waiter and does not strand the worker.
    for status in statuses[3:5]:
        assert status["type"] == "ResolverBoundaryError"
        assert "getaddrinfo refused" in status["diagnostic"]
    assert "unsupported resolver error identity EAI_SYSTEM" in statuses[3]["diagnostic"]
    assert "unsupported resolver error identity EAI_VENDOR" in statuses[4]["diagnostic"]


def test_node_socket_error_prefix_executes_actual_import_and_native_reset(
    tmp_path: Path,
) -> None:
    script = r"""
const fs = require('node:fs');
const Module = require('node:module');
const path = require('node:path');
const {Worker} = require('node:worker_threads');
const server = new Worker(`
  const {parentPort} = require('node:worker_threads');
  const net = require('node:net');
  const server = net.createServer(socket => socket.once('data', bytes => {
    socket.resetAndDestroy();
    parentPort.postMessage({type: 'reset', byte: bytes[0]});
  }));
  server.listen(0, '127.0.0.1', () => parentPort.postMessage(server.address().port));
`, {eval: true});
const filename = path.resolve('wasm/run_wasm.js');
const loaded = new Module(filename, module);
loaded.filename = filename;
loaded.paths = Module._nodeModulePaths(path.dirname(filename));
loaded._compile(fs.readFileSync(filename, 'utf8') + '\nmodule.exports.socket = { socketHostNew, socketHostConnect, socketHostWait, socketHostGetsockopt, socketHostSend, socketHostClose, setWasmMemory, close: () => socketWorkerClient.close() };', filename);
const host = loaded.exports.socket;
(async () => {
  try {
    const port = await new Promise((resolve, reject) => { server.once('message', resolve); server.once('error', reject); });
    const text = value => [...Buffer.from(value)];
    const name = value => [text(value).length, ...text(value)];
    const section = (id, body) => [id, body.length, ...body];
    const bytes = [0,97,115,109,1,0,0,0,
      ...section(1, [1, 0x60, 6, 0x7e, ...Array(5).fill(0x7f), 1, 0x7f]),
      ...section(2, [2, ...name('env'), ...name('molt_socket_getsockopt_host'), 0, 0, ...name('env'), ...name('memory'), 2, 0, 1]),
      ...section(7, [2, ...name('get'), 0, 0, ...name('memory'), 2, 0])];
    const memory = new WebAssembly.Memory({initial: 1});
    host.setWasmMemory(memory);
    const {instance} = await WebAssembly.instantiate(new Uint8Array(bytes), {env: {memory, molt_socket_getsockopt_host: host.socketHostGetsockopt}});
    const view = new DataView(memory.buffer);
    const results = [];
    for (const capacity of [1, 4]) {
      const reset = new Promise((resolve, reject) => { server.once('message', resolve); server.once('error', reject); });
      const handle = host.socketHostNew(2, 1, 6, -1);
      try {
        new Uint8Array(memory.buffer).set([2, 0, port & 255, port >> 8, 127, 0, 0, 1], 64);
        const connected = host.socketHostConnect(handle, 64, 8);
        if (connected !== 0 && connected !== -115) throw new Error(`connect failed ${connected}`);
        if (host.socketHostWait(handle, 2, 2000) < 0) throw new Error('connect readiness failed');
        new Uint8Array(memory.buffer)[512] = 7;
        if (host.socketHostSend(handle, 512, 1, 0) !== 1) throw new Error('handshake byte was not sent');
        const observed = await reset;
        if (observed.type !== 'reset' || observed.byte !== 7) throw new Error('server did not reset after the actual handshake byte');
        if (host.socketHostWait(handle, 1, 2000) < 0) throw new Error('reset readiness failed');
        new Uint8Array(memory.buffer).set([165,165,165,165], 128);
        const status = instance.exports.get(handle, 1, 4, 128, capacity, 256);
        results.push({status, length: view.getUint32(256, true), bytes: [...new Uint8Array(memory.buffer, 128, 4)]});
        if (instance.exports.get(handle, 1, 4, 128, -1, 256) !== -22) throw new Error('negative option capacity accepted');
        if (instance.exports.get(handle, 1, 4, 128, 4, 65535) !== -14) throw new Error('invalid option length pointer accepted');
      } finally { host.socketHostClose(handle); }
    }
    console.log(JSON.stringify(results));
  } finally { await server.terminate(); await host.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
"""
    result = _run_wasm_test_process(
        ["node", "-e", script], role="guest-host", cwd=ROOT, env=os.environ, timeout=30
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == [
        {"status": 0, "length": 1, "bytes": [104, 165, 165, 165]},
        {"status": 0, "length": 4, "bytes": [104, 0, 0, 0]},
    ]


def test_deployment_launch_transport_and_overlay_use_one_host_contract():
    from copy import deepcopy
    from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_VERSION
    from molt.wasm_artifact import require_current_wasm_execution_manifest

    environment = {
        "MOLT_CAPABILITIES": "fs.read",
        "MOLT_CAPABILITY_TIER": "none",
        "MOLT_CAPABILITY_POLICY_DIGEST": "sha256:" + "a" * 64,
        "MOLT_RESOURCE_MAX_DURATION_MS": "23",
        "__proto__": "ordinary environment value",
    }
    valid = {
        "version": WASM_EXECUTION_MANIFEST_VERSION,
        "launch": {"environment": environment},
    }
    invalid_launches = [
        None,
        [],
        {},
        {"environment": []},
        {"environment": environment, "fallback": True},
    ]
    for name in (
        "MOLT_CAPABILITIES",
        "MOLT_CAPABILITY_TIER",
        "MOLT_CAPABILITY_POLICY_DIGEST",
    ):
        missing = dict(environment)
        del missing[name]
        invalid_launches.append({"environment": missing})
    for name, value in (
        ("", "v"),
        ("bad=name", "v"),
        ("bad\0name", "v"),
        ("EXTRA", None),
        ("EXTRA", 1),
        ("EXTRA", "bad\0value"),
    ):
        invalid_launches.append({"environment": {**environment, name: value}})
    for launch in invalid_launches:
        with pytest.raises(ValueError, match="launch|environment"):
            require_current_wasm_execution_manifest({**valid, "launch": launch})
    before = deepcopy(valid)
    assert require_current_wasm_execution_manifest(valid) is valid
    assert valid == before
    script = (
        "const valid = JSON.parse("
        + json.dumps(json.dumps(valid))
        + ");\nconst invalid = JSON.parse("
        + json.dumps(json.dumps(invalid_launches))
        + r""");
const assert = require('node:assert/strict');
const {requireMoltDeploymentManifest, deploymentLaunchEnvironment} = require('./wasm/loader_bridge.js');
for (const launch of invalid) {
  assert.throws(() => requireMoltDeploymentManifest({...valid, launch}), /launch|environment/);
}
const ambient = {MOLT_CAPABILITIES: '', MOLT_CAPABILITY_TIER: 'full',
  MOLT_RESOURCE_MAX_MEMORY: '17', MOLT_RESOURCE_MAX_DURATION_MS: '99',
  MOLT_EXECUTION_TARGET: 'forged', UNRELATED: 'preserved'};
const before = JSON.stringify(ambient);
const env = deploymentLaunchEnvironment(valid, ambient);
assert.equal(env.MOLT_CAPABILITIES, 'fs.read');
assert.equal(env.MOLT_CAPABILITY_TIER, 'none');
assert.equal(env.MOLT_RESOURCE_MAX_DURATION_MS, '23');
assert.equal(env.MOLT_RESOURCE_MAX_MEMORY, '17');
assert.equal(env.UNRELATED, 'preserved');
assert.equal(env.__proto__, 'ordinary environment value');
assert.equal(Object.getPrototypeOf(env), null);
assert.equal(JSON.stringify(ambient), before);
console.log('policy overlay and refusals passed');
"""
    )
    result = _run_wasm_test_process(
        ["node", "-e", script], role="tool", cwd=ROOT, env=os.environ, timeout=30
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "policy overlay and refusals passed"


@pytest.mark.parametrize("capabilities", ["", "fs.read"])
def test_public_node_and_browser_deliver_declared_policy_to_wasi(
    tmp_path, capabilities
):
    from molt.capability_manifest import resolve_runtime_policy_from_env

    wasm_tools = shutil.which("wasm-tools")
    if wasm_tools is None:
        pytest.skip("wasm-tools is required for the WASI environment fixture")
    wat = tmp_path / "environment.wat"
    wasm = tmp_path / "environment.wasm"
    wat.write_text(
        r"""
(module
  (import "wasi_snapshot_preview1" "environ_sizes_get" (func $sizes (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "environ_get" (func $get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write" (func $write (param i32 i32 i32 i32) (result i32)))
  (memory (export "molt_memory") 2)
  (table (export "molt_table") 1 funcref)
  (func (export "molt_runtime_execution_enter") (result i64) i64.const 1)
  (func (export "molt_runtime_execution_leave") (param i64))
  (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
  (func (export "molt_exception_pending") (result i64) i64.const 0)
  (func (export "molt_main_exit_status") (result i64) i64.const 0)
  (func (export "molt_main")
    i32.const 0 i32.const 4 call $sizes if unreachable end
    i32.const 1024 i32.const 8192 call $get if unreachable end
    i32.const 512 i32.const 8192 i32.store
    i32.const 516 i32.const 4 i32.load i32.store
    i32.const 1 i32.const 512 i32.const 1 i32.const 520 call $write if unreachable end))
""",
        encoding="utf-8",
    )
    parsed = _run_wasm_test_process(
        [wasm_tools, "parse", str(wat), "-o", str(wasm)],
        role="tool",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert parsed.returncode == 0, parsed.stderr
    _attest_empty_callable_table(wasm)
    policy = resolve_runtime_policy_from_env(
        {
            "MOLT_CAPABILITIES": capabilities,
            "MOLT_CAPABILITY_TIER": "none",
        }
    )
    manifest = write_wasm_execution_manifest(tmp_path, linked=wasm, policy=policy)
    payload = json.loads(manifest.read_text())
    payload["abi"] = {
        "linked_self_imports": [],
        "runtime_imports": {
            "names": [
                "runtime_execution_enter",
                "runtime_execution_leave",
                "runtime_shutdown",
            ],
            "export_names": {
                "runtime_execution_enter": "molt_runtime_execution_enter",
                "runtime_execution_leave": "molt_runtime_execution_leave",
                "runtime_shutdown": "molt_runtime_shutdown",
            },
        },
    }
    manifest.write_text(json.dumps(payload), encoding="utf-8")
    ambient = {
        **{
            key: os.environ[key]
            for key in (
                "PATH",
                "MOLT_EXT_ROOT",
                "MOLT_TARGET_ROOT",
                "MOLT_NODE_BIN",
                "CARGO_TARGET_DIR",
                "TMPDIR",
            )
            if key in os.environ
        },
        "MOLT_CAPABILITY_TIER": "full",
        "MOLT_CAPABILITIES": "env.read",
        "MOLT_RESOURCE_MAX_MEMORY": "17000000",
        "MOLT_EXECUTION_TARGET": "forged",
    }
    direct = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(manifest)],
        role="guest-host",
        cwd=ROOT,
        env=ambient,
        timeout=30,
    )
    assert direct.returncode == 0, direct.stderr
    entries = direct.stdout.split("\0")
    assert f"MOLT_CAPABILITIES={capabilities}" in entries
    assert "MOLT_CAPABILITY_TIER=none" in entries
    assert "MOLT_RESOURCE_MAX_MEMORY=17000000" in entries
    assert "MOLT_EXECUTION_TARGET=wasi" in entries
    assert "MOLT_CAPABILITIES=env.read" not in entries
    script = (
        "const manifestPath = "
        + json.dumps(str(manifest))
        + r""";
(async () => {
  const fs = require('node:fs');
  const {pathToFileURL, fileURLToPath} = require('node:url');
  globalThis.fetch = async url => new Response(fs.readFileSync(fileURLToPath(url)));
  const {loadMoltWasm} = await import(pathToFileURL(require('node:path').resolve('wasm/browser_host.js')).href);
  const instance = await loadMoltWasm({manifestUrl: pathToFileURL(manifestPath).href, vfs: {},
    env: {MOLT_CAPABILITY_TIER: 'full', MOLT_CAPABILITIES: 'env.read',
      MOLT_RESOURCE_MAX_MEMORY: '17000000', MOLT_EXECUTION_TARGET: 'forged'}});
  try {
    instance.run();
    const memory = instance.memory.buffer;
    const size = new DataView(memory).getUint32(4, true);
    const entries = new TextDecoder().decode(new Uint8Array(memory, 8192, size)).split('\0');
    console.log(JSON.stringify(entries));
  } finally { instance.dispose(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
"""
    )
    browser = _run_wasm_test_process(
        ["node", "-e", script],
        role="tool",
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert browser.returncode == 0, browser.stderr
    entries = json.loads(browser.stdout.strip().splitlines()[-1])
    assert f"MOLT_CAPABILITIES={capabilities}" in entries
    assert "MOLT_CAPABILITY_TIER=none" in entries
    assert "MOLT_RESOURCE_MAX_MEMORY=17000000" in entries
    assert "MOLT_EXECUTION_TARGET=browser" in entries
    assert "MOLT_CAPABILITIES=env.read" not in entries
