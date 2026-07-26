import ast
import os
import re
import shutil
from pathlib import Path

import pytest

from tests.wasm_linked_runner import _run_wasm_test_process
from tests.wasm_harness import BASE_PREAMBLE, IMPORT_HELPERS
from tests.wasm_import_fixtures import build_wasm_tag_import_before_memory
from molt._wasm_abi_generated import wasm_runtime_import_name

ROOT = Path(__file__).resolve().parents[1]


def _encode_u32(value: int) -> bytes:
    out = bytearray()
    remaining = value
    while True:
        byte = remaining & 0x7F
        remaining >>= 7
        if remaining:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            break
    return bytes(out)


def _encode_str(text: str) -> bytes:
    data = text.encode("utf-8")
    return _encode_u32(len(data)) + data


def _build_wasm_global_get_data_offset() -> bytes:
    magic = b"\x00asm"
    version = b"\x01\x00\x00\x00"

    import_entries = bytearray()
    import_entries += _encode_str("env")
    import_entries += _encode_str("memory")
    import_entries.append(0x02)  # memory import
    import_entries.append(0x00)  # limits: min only
    import_entries += _encode_u32(1)  # 1 page

    import_entries += _encode_str("env")
    import_entries += _encode_str("__memory_base")
    import_entries.append(0x03)  # global import
    import_entries.append(0x7F)  # i32
    import_entries.append(0x00)  # immutable

    import_payload = _encode_u32(2) + bytes(import_entries)
    import_section = bytes([0x02]) + _encode_u32(len(import_payload)) + import_payload

    data_bytes = b"A"
    data_segment = bytearray()
    data_segment.append(0x00)  # active segment, memory index 0
    data_segment.append(0x23)  # global.get
    data_segment += _encode_u32(0)  # global index 0
    data_segment.append(0x0B)  # end
    data_segment += _encode_u32(len(data_bytes))
    data_segment += data_bytes

    data_payload = _encode_u32(1) + bytes(data_segment)
    data_section = bytes([0x0B]) + _encode_u32(len(data_payload)) + data_payload

    return magic + version + import_section + data_section


def _build_wasm_const_data_offset() -> bytes:
    magic = b"\x00asm"
    version = b"\x01\x00\x00\x00"

    memory_entry = bytearray()
    memory_entry.append(0x00)  # limits: min only
    memory_entry += _encode_u32(1)  # 1 page
    memory_payload = _encode_u32(1) + bytes(memory_entry)
    memory_section = bytes([0x05]) + _encode_u32(len(memory_payload)) + memory_payload

    data_bytes = b"WASM"
    data_segment = bytearray()
    data_segment.append(0x00)  # active segment, memory index 0
    data_segment.append(0x41)  # i32.const
    data_segment += _encode_u32(128)
    data_segment.append(0x0B)  # end
    data_segment += _encode_u32(len(data_bytes))
    data_segment += data_bytes

    data_payload = _encode_u32(1) + bytes(data_segment)
    data_section = bytes([0x0B]) + _encode_u32(len(data_payload)) + data_payload

    return magic + version + memory_section + data_section


def test_wasm_harness_data_end_handles_global_get(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm harness test")

    wasm_path = tmp_path / "global_get_data_offset.wasm"
    wasm_path.write_bytes(_build_wasm_global_get_data_offset())

    runner = tmp_path / "parse_wasm_data_end.js"
    runner.write_text(
        BASE_PREAMBLE
        + "\n"
        + IMPORT_HELPERS
        + "\n"
        + "console.log(`dataEnd=${wasmDataEnd}`);\n"
    )

    run = _run_wasm_test_process(
        ["node", str(runner), str(wasm_path)],
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "dataEnd=65536"


def test_wasm_harness_data_end_handles_const_offset(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm harness test")

    wasm_path = tmp_path / "const_data_offset.wasm"
    wasm_path.write_bytes(_build_wasm_const_data_offset())

    runner = tmp_path / "parse_wasm_data_end_const.js"
    runner.write_text(
        BASE_PREAMBLE
        + "\n"
        + IMPORT_HELPERS
        + "\n"
        + "console.log(`dataEnd=${wasmDataEnd}`);\n"
    )

    run = _run_wasm_test_process(
        ["node", str(runner), str(wasm_path)],
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "dataEnd=132"


def test_wasm_harness_import_parser_handles_tag_imports_before_memory(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm harness test")

    wasm_path = tmp_path / "tag_import_before_memory.wasm"
    wasm_path.write_bytes(build_wasm_tag_import_before_memory())

    runner = tmp_path / "parse_wasm_tag_imports.js"
    runner.write_text(
        BASE_PREAMBLE
        + "\n"
        + IMPORT_HELPERS
        + "\n"
        + "console.log(JSON.stringify(wasmImports));\n"
    )

    run = _run_wasm_test_process(
        ["node", str(runner), str(wasm_path)],
        cwd=ROOT,
        env=os.environ,
        timeout=30,
    )
    assert run.returncode == 0, run.stderr
    assert '"memory":{"min":1,"max":null}' in run.stdout.strip()


def test_wasm_harness_exposes_class_merge_layout_import() -> None:
    source = Path(__file__).resolve().parent / "wasm_harness.py"
    assert (
        "class_merge_layout: (classBits, offsetsBits, sizeBits) => {"
        in source.read_text()
    )


def test_wasm_harness_exposes_string_split_field_imports() -> None:
    source = Path(__file__).resolve().parent / "wasm_harness.py"
    text = source.read_text()
    assert "string_split_validate: (hayBits, needleBits) => {" in text
    assert "string_split_field: (hayBits, needleBits, indexBits) => {" in text
    assert "string_split_field_len: (hayBits, needleBits, indexBits) => {" in text
    assert (
        "string_split_field_eq: (hayBits, needleBits, indexBits, expectedBits) => {"
        in text
    )


def test_wasm_harness_exposes_ord_at_import() -> None:
    source = Path(__file__).resolve().parent / "wasm_harness.py"
    text = source.read_text()
    assert "ord_at: (objBits, idxBits) => {" in text
    assert "return boxInt(BigInt(chars[pos].codePointAt(0)))" in text


def test_wasm_harness_implements_private_thread_intrinsic_family() -> None:
    thread_source = ROOT / "src/molt/stdlib/_thread.py"
    tree = ast.parse(thread_source.read_text(encoding="utf-8"))
    runtime_names = {
        node.args[0].value
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "_require_intrinsic"
        and node.args
        and isinstance(node.args[0], ast.Constant)
        and isinstance(node.args[0].value, str)
    }
    import_names = {
        import_name
        for runtime_name in runtime_names
        if (import_name := wasm_runtime_import_name(runtime_name)) is not None
    }
    harness = (ROOT / "tests/wasm_harness.py").read_text(encoding="utf-8")
    missing = sorted(
        name
        for name in import_names
        if re.search(rf"^\s+{re.escape(name)}:\s*", harness, re.MULTILINE) is None
    )
    assert not missing, f"WASM harness lacks _thread runtime imports: {missing}"
    assert "const mainThreadIdent = 1;" in harness
    assert "thread_current_ident: () => boxInt(mainThreadIdent)" in harness
    assert "thread_current_native_id: () => boxInt(mainThreadIdent)" in harness
    assert harness.count("threads are unavailable in wasm") >= 2
