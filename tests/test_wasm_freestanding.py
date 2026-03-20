"""Tests for wasm-freestanding target and WASI import stubbing."""
from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parents[1]
FIXTURE = PROJECT_ROOT / "tests" / "fixtures" / "freestanding_hello.py"


def _load_stub_module():
    path = PROJECT_ROOT / "tools" / "wasm_stub_wasi.py"
    spec = importlib.util.spec_from_file_location("wasm_stub_wasi", path)
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


stub_mod = _load_stub_module()


def _read_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = shift = 0
    while True:
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if not (byte & 0x80):
            break
        shift += 7
    return result, offset


def _read_string(data: bytes, offset: int) -> tuple[str, int]:
    length, offset = _read_varuint(data, offset)
    return data[offset : offset + length].decode(), offset + length


def _read_limits(data: bytes, offset: int) -> tuple[int, int]:
    flags = data[offset]
    offset += 1
    _, offset = _read_varuint(data, offset)
    if flags & 1:
        _, offset = _read_varuint(data, offset)
    return flags, offset


def _parse_wasm_imports(wasm_bytes: bytes) -> list[tuple[str, str]]:
    """Extract (module, name) pairs from a WASM import section."""
    if len(wasm_bytes) < 8 or wasm_bytes[:4] != b"\x00asm":
        return []
    offset = 8
    imports: list[tuple[str, str]] = []
    while offset < len(wasm_bytes):
        section_id = wasm_bytes[offset]
        offset += 1
        size, offset = _read_varuint(wasm_bytes, offset)
        section_end = offset + size
        if section_id == 2:  # Import section
            count, offset = _read_varuint(wasm_bytes, offset)
            for _ in range(count):
                mod_name, offset = _read_string(wasm_bytes, offset)
                field_name, offset = _read_string(wasm_bytes, offset)
                kind = wasm_bytes[offset]
                offset += 1
                if kind == 0:  # func
                    _, offset = _read_varuint(wasm_bytes, offset)
                elif kind == 1:  # table
                    offset += 1
                    _, offset = _read_limits(wasm_bytes, offset)
                elif kind == 2:  # memory
                    _, offset = _read_limits(wasm_bytes, offset)
                elif kind == 3:  # global
                    offset += 2
                imports.append((mod_name, field_name))
            break
        offset = section_end
    return imports


# ---------------------------------------------------------------------------
# Unit tests for wasm_stub_wasi.py
# ---------------------------------------------------------------------------


def _build_minimal_wasm_with_wasi_import() -> bytes:
    """Build a minimal valid WASM module with one WASI import and one defined function."""
    w = stub_mod._write_varuint
    ws = stub_mod._write_string

    sections: list[tuple[int, bytes]] = []

    # Type section: one type () -> ()
    type_payload = bytearray()
    type_payload.extend(w(1))  # 1 type
    type_payload.append(0x60)  # functype
    type_payload.extend(w(0))  # 0 params
    type_payload.extend(w(0))  # 0 results
    sections.append((1, bytes(type_payload)))

    # Import section: one WASI import (func type 0)
    import_payload = bytearray()
    import_payload.extend(w(1))  # 1 import
    import_payload.extend(ws("wasi_snapshot_preview1"))
    import_payload.extend(ws("fd_write"))
    import_payload.append(0x00)  # kind: function
    import_payload.extend(w(0))  # type index 0
    sections.append((2, bytes(import_payload)))

    # Function section: one defined function (type 0)
    func_payload = w(1) + w(0)
    sections.append((3, bytes(func_payload)))

    # Code section: one function body (nop; end)
    body = bytes([0x00, 0x01, 0x0B])  # 0 locals, nop, end
    code_payload = bytearray()
    code_payload.extend(w(1))  # 1 function
    code_payload.extend(w(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return stub_mod._build_sections(sections)


def test_stub_wasi_removes_wasi_imports():
    """stub_wasi_imports should remove all wasi_snapshot_preview1 imports."""
    wasm = _build_minimal_wasm_with_wasi_import()
    imports_before = _parse_wasm_imports(wasm)
    assert any(mod == "wasi_snapshot_preview1" for mod, _ in imports_before)

    result, n_stubbed = stub_mod.stub_wasi_imports(wasm)
    assert n_stubbed == 1
    imports_after = _parse_wasm_imports(result)
    wasi_after = [(m, n) for m, n in imports_after if m == "wasi_snapshot_preview1"]
    assert wasi_after == [], f"WASI imports remain: {wasi_after}"


def test_stub_wasi_with_non_function_imports():
    """WASI stubbing must work when table/memory imports precede WASI function imports."""
    w = stub_mod._write_varuint
    ws = stub_mod._write_string

    sections: list[tuple[int, bytes]] = []

    # Type section: one type () -> ()
    type_payload = bytearray()
    type_payload.extend(w(1))
    type_payload.append(0x60)
    type_payload.extend(w(0))
    type_payload.extend(w(0))
    sections.append((1, bytes(type_payload)))

    # Import section: 1 table import (env.__indirect_function_table) + 1 WASI func import
    import_payload = bytearray()
    import_payload.extend(w(2))  # 2 imports
    # Table import
    import_payload.extend(ws("env"))
    import_payload.extend(ws("__indirect_function_table"))
    import_payload.append(0x01)  # kind: table
    import_payload.append(0x70)  # funcref
    import_payload.append(0x00)  # limits flags (no max)
    import_payload.extend(w(0))  # min = 0
    # WASI function import
    import_payload.extend(ws("wasi_snapshot_preview1"))
    import_payload.extend(ws("fd_write"))
    import_payload.append(0x00)  # kind: function
    import_payload.extend(w(0))  # type index 0
    sections.append((2, bytes(import_payload)))

    # Function section: one defined function (type 0)
    func_payload = w(1) + w(0)
    sections.append((3, bytes(func_payload)))

    # Code section: one function body that calls the WASI import (func idx 0)
    body = bytes([0x00, 0x10, 0x00, 0x0B])  # 0 locals, call 0, end
    code_payload = bytearray()
    code_payload.extend(w(1))
    code_payload.extend(w(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    wasm = stub_mod._build_sections(sections)
    result, n_stubbed = stub_mod.stub_wasi_imports(wasm)
    assert n_stubbed == 1

    imports_after = _parse_wasm_imports(result)
    wasi_after = [(m, n) for m, n in imports_after if m == "wasi_snapshot_preview1"]
    assert wasi_after == [], f"WASI imports remain after stubbing: {wasi_after}"

    # Table import should still be present
    table_imports = [(m, n) for m, n in imports_after if n == "__indirect_function_table"]
    assert len(table_imports) == 1, "Table import should survive stubbing"


def test_stub_wasi_no_wasi_imports_is_noop():
    """If there are no WASI imports, output should be identical to input."""
    w = stub_mod._write_varuint

    sections: list[tuple[int, bytes]] = []
    type_payload = bytearray()
    type_payload.extend(w(1))
    type_payload.append(0x60)
    type_payload.extend(w(0))
    type_payload.extend(w(0))
    sections.append((1, bytes(type_payload)))

    func_payload = w(1) + w(0)
    sections.append((3, bytes(func_payload)))

    body = bytes([0x00, 0x01, 0x0B])
    code_payload = bytearray()
    code_payload.extend(w(1))
    code_payload.extend(w(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    wasm = stub_mod._build_sections(sections)
    result, n_stubbed = stub_mod.stub_wasi_imports(wasm)
    assert n_stubbed == 0
    assert result == wasm


def test_stub_wasi_output_is_valid_wasm():
    """The stubbed output must be valid WASM."""
    wasm = _build_minimal_wasm_with_wasi_import()
    result, _ = stub_mod.stub_wasi_imports(wasm)
    assert result[:4] == b"\x00asm"
    assert result[4:8] == b"\x01\x00\x00\x00"


# ---------------------------------------------------------------------------
# End-to-end tests (require full build toolchain)
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_freestanding_produces_no_wasi_imports(tmp_path):
    """A freestanding build must contain zero wasi_snapshot_preview1 imports."""
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            str(FIXTURE),
            "--target",
            "wasm-freestanding",
            "--output",
            str(output),
            "--linked-output",
            str(linked),
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=180,
    )
    assert (
        result.returncode == 0
    ), f"Build failed:\nstdout: {result.stdout}\nstderr: {result.stderr}"
    wasm_bytes = linked.read_bytes()
    imports = _parse_wasm_imports(wasm_bytes)
    wasi_imports = [
        (mod, name) for mod, name in imports if mod == "wasi_snapshot_preview1"
    ]
    assert wasi_imports == [], f"Freestanding binary has WASI imports: {wasi_imports}"


@pytest.mark.slow
def test_freestanding_binary_is_valid_wasm(tmp_path):
    """The linked freestanding binary must be valid WASM."""
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            str(FIXTURE),
            "--target",
            "wasm-freestanding",
            "--output",
            str(output),
            "--linked-output",
            str(linked),
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=180,
    )
    assert (
        result.returncode == 0
    ), f"Build failed:\nstdout: {result.stdout}\nstderr: {result.stderr}"
    wasm_bytes = linked.read_bytes()
    assert wasm_bytes[:4] == b"\x00asm", "Not a valid WASM binary"
    assert wasm_bytes[4:8] == b"\x01\x00\x00\x00", "Not WASM version 1"
