from __future__ import annotations

import json
import struct
from pathlib import Path

import pytest
from molt.gpu.interop import load_safetensors_bytes
from tools.convert_safetensors_dtype import convert_safetensors_dtype

pytest.importorskip("numpy")


def _write_safetensors_fixture(path: Path) -> None:
    header = {
        "__metadata__": {"format": "unit-test"},
        "f32": {
            "dtype": "F32",
            "shape": [2],
            "data_offsets": [0, 8],
        },
        "i64": {
            "dtype": "I64",
            "shape": [2],
            "data_offsets": [8, 24],
        },
    }
    f32_raw = struct.pack("<2f", 1.0, -3.5)
    i64_raw = struct.pack("<2q", 7, 9)
    header_bytes = json.dumps(header, separators=(",", ":")).encode("utf-8")
    with path.open("wb") as handle:
        handle.write(struct.pack("<Q", len(header_bytes)))
        handle.write(header_bytes)
        handle.write(f32_raw)
        handle.write(i64_raw)


def _read_header(path: Path) -> dict[str, object]:
    data = path.read_bytes()
    header_len = struct.unpack_from("<Q", data, 0)[0]
    return json.loads(data[8 : 8 + header_len].decode("utf-8"))


def test_convert_safetensors_dtype_rewrites_float_dtypes_and_offsets(
    tmp_path: Path,
) -> None:
    src = tmp_path / "weights.safetensors"
    out = tmp_path / "weights_f16.safetensors"
    _write_safetensors_fixture(src)

    stats = convert_safetensors_dtype(src, out, target_dtype="F16")

    header = _read_header(out)
    assert header["__metadata__"] == {"format": "unit-test"}
    assert header["f32"]["dtype"] == "F16"
    assert header["f32"]["data_offsets"] == [0, 4]
    assert header["i64"]["dtype"] == "I64"
    assert header["i64"]["data_offsets"] == [4, 20]
    assert stats.converted_tensors == 1
    assert stats.preserved_tensors == 1
    assert stats.output_bytes < stats.input_bytes


def test_convert_safetensors_dtype_output_remains_loadable(tmp_path: Path) -> None:
    src = tmp_path / "weights.safetensors"
    out = tmp_path / "weights_f16.safetensors"
    _write_safetensors_fixture(src)

    convert_safetensors_dtype(src, out, target_dtype="F16")
    weights = load_safetensors_bytes(out.read_bytes())

    assert [round(v, 3) for v in weights["f32"]._data_list()] == [1.0, -3.5]
    assert weights["i64"]._data_list() == [7, 9]
