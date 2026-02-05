import shutil
import subprocess
from pathlib import Path

import pytest

from tests.wasm_harness import BASE_PREAMBLE, IMPORT_HELPERS


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

    run = subprocess.run(
        ["node", str(runner), str(wasm_path)],
        capture_output=True,
        text=True,
        check=False,
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

    run = subprocess.run(
        ["node", str(runner), str(wasm_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "dataEnd=132"
