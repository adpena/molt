"""Tests for wasm-freestanding target and WASI import stubbing."""
from __future__ import annotations

import importlib.util
import shutil
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


def _load_wasm_link_module():
    path = PROJECT_ROOT / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link", path)
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


wasm_link_mod = _load_wasm_link_module()


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


def _parse_code_section_call_targets(wasm_bytes: bytes) -> list[list[int]]:
    """Extract call instruction targets from each function body in the code section.

    Returns a list of lists: one inner list per function body, containing
    the function indices targeted by ``call`` (0x10) instructions.
    """
    offset = 8  # skip WASM header
    while offset < len(wasm_bytes):
        section_id = wasm_bytes[offset]
        offset += 1
        size, offset = _read_varuint(wasm_bytes, offset)
        section_end = offset + size
        if section_id == 10:  # Code section
            count, offset = _read_varuint(wasm_bytes, offset)
            result: list[list[int]] = []
            for _ in range(count):
                body_size, offset = _read_varuint(wasm_bytes, offset)
                body_end = offset + body_size
                calls: list[int] = []
                # Skip local declarations
                n_local_decls, pos = _read_varuint(wasm_bytes, offset)
                for _ in range(n_local_decls):
                    _, pos = _read_varuint(wasm_bytes, pos)
                    pos += 1  # valtype
                # Scan for call instructions
                while pos < body_end:
                    opcode = wasm_bytes[pos]
                    pos += 1
                    if opcode == 0x10:  # call
                        func_idx, pos = _read_varuint(wasm_bytes, pos)
                        calls.append(func_idx)
                    elif opcode == 0x0B:  # end
                        pass
                    elif opcode == 0x00:  # unreachable
                        pass
                    elif opcode == 0x01:  # nop
                        pass
                result.append(calls)
                offset = body_end
            return result
        offset = section_end
    return []


def _parse_wasm_exports(wasm_bytes: bytes) -> list[tuple[str, int, int]]:
    """Extract (name, kind, index) tuples from the WASM export section."""
    offset = 8
    while offset < len(wasm_bytes):
        section_id = wasm_bytes[offset]
        offset += 1
        size, offset = _read_varuint(wasm_bytes, offset)
        section_end = offset + size
        if section_id == 7:  # Export section
            count, offset = _read_varuint(wasm_bytes, offset)
            exports: list[tuple[str, int, int]] = []
            for _ in range(count):
                name, offset = _read_string(wasm_bytes, offset)
                kind = wasm_bytes[offset]
                offset += 1
                idx, offset = _read_varuint(wasm_bytes, offset)
                exports.append((name, kind, idx))
            return exports
        offset = section_end
    return []


def _count_wasm_sections(wasm_bytes: bytes) -> int:
    """Count the number of sections in a WASM binary."""
    offset = 8
    count = 0
    while offset < len(wasm_bytes):
        offset += 1  # section id
        size, offset = _read_varuint(wasm_bytes, offset)
        offset += size
        count += 1
    return count


def test_stub_wasi_remaps_call_targets():
    """Call instructions must be remapped after WASI imports are stubbed out."""
    w = stub_mod._write_varuint
    ws = stub_mod._write_string

    sections: list[tuple[int, bytes]] = []

    # Type section: one type () -> ()
    type_payload = bytearray()
    type_payload.extend(w(1))  # 1 type
    type_payload.append(0x60)
    type_payload.extend(w(0))
    type_payload.extend(w(0))
    sections.append((1, bytes(type_payload)))

    # Import section: 2 function imports
    #   import 0: wasi_snapshot_preview1.fd_write (func, type 0) -> func idx 0
    #   import 1: env.helper (func, type 0) -> func idx 1
    import_payload = bytearray()
    import_payload.extend(w(2))
    import_payload.extend(ws("wasi_snapshot_preview1"))
    import_payload.extend(ws("fd_write"))
    import_payload.append(0x00)
    import_payload.extend(w(0))
    import_payload.extend(ws("env"))
    import_payload.extend(ws("helper"))
    import_payload.append(0x00)
    import_payload.extend(w(0))
    sections.append((2, bytes(import_payload)))

    # Function section: one defined function (type 0) -> func idx 2
    func_payload = w(1) + w(0)
    sections.append((3, bytes(func_payload)))

    # Code section: defined func calls func idx 0 (WASI) then func idx 1 (env)
    body = bytearray()
    body.append(0x00)  # 0 local declarations
    body.append(0x10)  # call
    body.extend(w(0))  # func idx 0 (fd_write)
    body.append(0x10)  # call
    body.extend(w(1))  # func idx 1 (helper)
    body.append(0x0B)  # end
    code_payload = bytearray()
    code_payload.extend(w(1))
    code_payload.extend(w(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    wasm = stub_mod._build_sections(sections)
    result, n_stubbed = stub_mod.stub_wasi_imports(wasm)
    assert n_stubbed == 1

    # After stubbing:
    #   import 0: env.helper (func idx 0) -- was at idx 1
    #   stub 0: fd_write stub (func idx 1) -- was at idx 0
    #   defined func 0 (func idx 2) -- unchanged
    # So call to old idx 0 -> new idx 1, call to old idx 1 -> new idx 0

    # Verify env import survived at its new index
    imports_after = _parse_wasm_imports(result)
    env_imports = [(m, n) for m, n in imports_after if m == "env"]
    assert len(env_imports) == 1
    assert env_imports[0] == ("env", "helper")

    # Verify call targets in the last function body (stubs come first, then original defined)
    call_targets = _parse_code_section_call_targets(result)
    # code section has: stub body (idx 0 in code), then original defined func (idx 1 in code)
    assert len(call_targets) == 2  # 1 stub + 1 original defined

    # The original defined function is the last body in the code section
    original_func_calls = call_targets[-1]
    assert len(original_func_calls) == 2
    # Old call 0 (WASI fd_write) should now target the stub at func idx 1
    assert original_func_calls[0] == 1, (
        f"Call to WASI import should target stub at idx 1, got {original_func_calls[0]}"
    )
    # Old call 1 (env.helper) should now target import at func idx 0
    assert original_func_calls[1] == 0, (
        f"Call to env import should target idx 0, got {original_func_calls[1]}"
    )


def test_stub_wasi_preserves_exports():
    """Exports must still point to the correct function after WASI stubbing."""
    w = stub_mod._write_varuint
    ws = stub_mod._write_string

    sections: list[tuple[int, bytes]] = []

    # Type section
    type_payload = bytearray()
    type_payload.extend(w(1))
    type_payload.append(0x60)
    type_payload.extend(w(0))
    type_payload.extend(w(0))
    sections.append((1, bytes(type_payload)))

    # Import section: 1 WASI import -> func idx 0
    import_payload = bytearray()
    import_payload.extend(w(1))
    import_payload.extend(ws("wasi_snapshot_preview1"))
    import_payload.extend(ws("fd_write"))
    import_payload.append(0x00)
    import_payload.extend(w(0))
    sections.append((2, bytes(import_payload)))

    # Function section: 1 defined func (type 0) -> func idx 1
    func_payload = w(1) + w(0)
    sections.append((3, bytes(func_payload)))

    # Export section: export defined func at func idx 1 as "main"
    export_payload = bytearray()
    export_payload.extend(w(1))  # 1 export
    export_payload.extend(ws("main"))
    export_payload.append(0x00)  # kind: function
    export_payload.extend(w(1))  # func idx 1
    sections.append((7, bytes(export_payload)))

    # Code section: 1 body (nop; end)
    body = bytes([0x00, 0x01, 0x0B])
    code_payload = bytearray()
    code_payload.extend(w(1))
    code_payload.extend(w(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    wasm = stub_mod._build_sections(sections)
    result, n_stubbed = stub_mod.stub_wasi_imports(wasm)
    assert n_stubbed == 1

    # After stubbing:
    #   stub 0: fd_write stub (func idx 0) -- replaces removed import
    #   defined func 0: func idx 1 -- unchanged
    # Export "main" should still point to func idx 1
    exports = _parse_wasm_exports(result)
    assert len(exports) == 1
    name, kind, idx = exports[0]
    assert name == "main"
    assert kind == 0  # function
    assert idx == 1, f"Export 'main' should point to func idx 1, got {idx}"


def test_stub_wasi_multiple_wasi_imports():
    """All WASI imports should be stubbed; non-WASI imports must survive."""
    w = stub_mod._write_varuint
    ws = stub_mod._write_string

    sections: list[tuple[int, bytes]] = []

    # Type section
    type_payload = bytearray()
    type_payload.extend(w(1))
    type_payload.append(0x60)
    type_payload.extend(w(0))
    type_payload.extend(w(0))
    sections.append((1, bytes(type_payload)))

    # Import section: 3 WASI imports + 1 env import
    #   import 0: wasi fd_write   (func idx 0)
    #   import 1: wasi fd_read    (func idx 1)
    #   import 2: wasi proc_exit  (func idx 2)
    #   import 3: env.helper      (func idx 3)
    import_payload = bytearray()
    import_payload.extend(w(4))
    for name in ("fd_write", "fd_read", "proc_exit"):
        import_payload.extend(ws("wasi_snapshot_preview1"))
        import_payload.extend(ws(name))
        import_payload.append(0x00)
        import_payload.extend(w(0))
    import_payload.extend(ws("env"))
    import_payload.extend(ws("helper"))
    import_payload.append(0x00)
    import_payload.extend(w(0))
    sections.append((2, bytes(import_payload)))

    # Function section: 1 defined func (type 0)
    func_payload = w(1) + w(0)
    sections.append((3, bytes(func_payload)))

    # Code section: nop; end
    body = bytes([0x00, 0x01, 0x0B])
    code_payload = bytearray()
    code_payload.extend(w(1))
    code_payload.extend(w(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    wasm = stub_mod._build_sections(sections)
    result, n_stubbed = stub_mod.stub_wasi_imports(wasm)
    assert n_stubbed == 3

    imports_after = _parse_wasm_imports(result)
    wasi_after = [(m, n) for m, n in imports_after if m == "wasi_snapshot_preview1"]
    assert wasi_after == [], f"WASI imports remain: {wasi_after}"

    env_after = [(m, n) for m, n in imports_after if m == "env"]
    assert len(env_after) == 1, f"env import should survive, got {env_after}"
    assert env_after[0] == ("env", "helper")


def test_stub_wasi_output_section_count_preserved():
    """Stubbing must not lose or duplicate sections."""
    wasm = _build_minimal_wasm_with_wasi_import()
    n_sections_before = _count_wasm_sections(wasm)

    result, n_stubbed = stub_mod.stub_wasi_imports(wasm)
    assert n_stubbed == 1

    n_sections_after = _count_wasm_sections(result)
    assert n_sections_after == n_sections_before, (
        f"Section count changed: {n_sections_before} -> {n_sections_after}"
    )


# ---------------------------------------------------------------------------
# wasm-validate integration test
# ---------------------------------------------------------------------------

_has_wasm_validate = shutil.which("wasm-validate") is not None


@pytest.mark.skipif(not _has_wasm_validate, reason="wasm-validate not installed")
def test_stub_wasi_output_validates():
    """Stubbed output must pass wasm-validate when the tool is available."""
    wasm = _build_minimal_wasm_with_wasi_import()
    result, n_stubbed = stub_mod.stub_wasi_imports(wasm)
    assert n_stubbed == 1

    valid, msg = stub_mod.validate_wasm(result)
    assert valid, f"wasm-validate failed on stubbed output: {msg}"


# ---------------------------------------------------------------------------
# _validate_freestanding tests
# ---------------------------------------------------------------------------


def _build_wasm_with_import(module_name: str, field_name: str) -> bytes:
    """Build a minimal valid WASM module with one function import."""
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

    # Import section: one import
    import_payload = bytearray()
    import_payload.extend(w(1))
    import_payload.extend(ws(module_name))
    import_payload.extend(ws(field_name))
    import_payload.append(0x00)  # kind: function
    import_payload.extend(w(0))  # type index 0
    sections.append((2, bytes(import_payload)))

    # Function section: one defined function (type 0)
    func_payload = w(1) + w(0)
    sections.append((3, bytes(func_payload)))

    # Code section: one function body (nop; end)
    body = bytes([0x00, 0x01, 0x0B])
    code_payload = bytearray()
    code_payload.extend(w(1))
    code_payload.extend(w(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return stub_mod._build_sections(sections)


def test_validate_freestanding_rejects_molt_runtime_import():
    """_validate_freestanding must fail when molt_runtime imports remain."""
    wasm = _build_wasm_with_import("molt_runtime", "some_func")
    assert not wasm_link_mod._validate_freestanding(wasm)


def test_validate_freestanding_rejects_wasi_import():
    """_validate_freestanding must fail when wasi_snapshot_preview1 imports remain."""
    wasm = _build_wasm_with_import("wasi_snapshot_preview1", "fd_write")
    assert not wasm_link_mod._validate_freestanding(wasm)


def test_validate_freestanding_accepts_env_import():
    """_validate_freestanding must accept imports from the env module."""
    wasm = _build_wasm_with_import("env", "__indirect_function_table")
    assert wasm_link_mod._validate_freestanding(wasm)


def test_validate_freestanding_warns_unknown_module(capsys):
    """_validate_freestanding should warn (not fail) for non-env imports."""
    wasm = _build_wasm_with_import("custom_host", "my_func")
    assert wasm_link_mod._validate_freestanding(wasm)
    captured = capsys.readouterr()
    assert "custom_host::my_func" in captured.err


def test_validate_freestanding_accepts_clean_module():
    """_validate_freestanding must accept a module with no imports at all."""
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
    assert wasm_link_mod._validate_freestanding(wasm)


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


@pytest.mark.slow
def test_precompile_produces_cwasm(tmp_path):
    """--precompile should produce a .cwasm alongside the .wasm."""
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
            "--precompile",
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=180,
    )
    assert (
        result.returncode == 0
    ), f"Build failed:\nstdout: {result.stdout}\nstderr: {result.stderr}"
    cwasm = linked.with_suffix(".cwasm")
    if shutil.which("wasmtime"):
        assert cwasm.exists(), f"Expected .cwasm at {cwasm}"
        assert cwasm.stat().st_size > 0, ".cwasm file is empty"
        assert "Precompiled to" in result.stderr
    else:
        # wasmtime not installed; precompilation should be skipped gracefully
        assert "wasmtime not found" in result.stderr


# ---------------------------------------------------------------------------
# --wasm-profile pure
# ---------------------------------------------------------------------------


def test_wasm_profile_pure_accepted():
    """--wasm-profile pure should be accepted by the CLI."""
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            "--help",
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=30,
    )
    assert result.returncode == 0
    assert "--profile" in result.stdout
    assert "cloudflare" in result.stdout


def test_profile_cloudflare_accepted():
    """--profile cloudflare should be accepted by the CLI."""
    result = subprocess.run(
        [sys.executable, "-m", "molt", "build", "--help"],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
    )
    assert "cloudflare" in result.stdout
