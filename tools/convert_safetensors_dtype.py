#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import struct
from dataclasses import dataclass
from pathlib import Path
from typing import Any

_FLOAT_DTYPE_NAMES = frozenset({"F16", "F32", "F64"})
_TARGET_DTYPE_NAMES = frozenset({"F16", "F32"})


def _numpy_module() -> Any:
    try:
        import numpy as np  # type: ignore
    except ModuleNotFoundError as exc:  # pragma: no cover - exercised via CLI error path
        raise RuntimeError(
            "convert_safetensors_dtype requires numpy; install it in the active environment"
        ) from exc
    return np


@dataclass(frozen=True)
class ConversionStats:
    input_bytes: int
    output_bytes: int
    converted_tensors: int
    preserved_tensors: int


def _load_safetensors_header(data: bytes) -> tuple[dict[str, Any], int]:
    if len(data) < 8:
        raise ValueError("SafeTensors payload is too small to contain a header")
    header_len = struct.unpack_from("<Q", data, 0)[0]
    if header_len > len(data) - 8:
        raise ValueError("SafeTensors header length exceeds file size")
    header_json = data[8 : 8 + header_len].decode("utf-8")
    return json.loads(header_json), 8 + header_len


def _is_tensor_record(record: Any) -> bool:
    return (
        isinstance(record, dict)
        and isinstance(record.get("dtype"), str)
        and isinstance(record.get("shape"), list)
        and isinstance(record.get("data_offsets"), list)
        and len(record["data_offsets"]) == 2
    )


def _convert_tensor_payload(raw: bytes, src_dtype: str, target_dtype: str) -> bytes:
    if src_dtype == target_dtype:
        return raw
    if src_dtype not in _FLOAT_DTYPE_NAMES or target_dtype not in _TARGET_DTYPE_NAMES:
        raise ValueError(f"Unsupported dtype conversion: {src_dtype} -> {target_dtype}")
    np = _numpy_module()
    np_src = {
        "F16": np.float16,
        "F32": np.float32,
        "F64": np.float64,
    }[src_dtype]
    np_target = {
        "F16": np.float16,
        "F32": np.float32,
    }[target_dtype]
    values = np.frombuffer(raw, dtype=np_src)
    converted = np.ascontiguousarray(values.astype(np_target))
    return converted.tobytes()


def convert_safetensors_dtype(
    input_path: Path,
    output_path: Path,
    *,
    target_dtype: str,
) -> ConversionStats:
    target_dtype = target_dtype.upper()
    if target_dtype not in _TARGET_DTYPE_NAMES:
        supported = ", ".join(sorted(_TARGET_DTYPE_NAMES))
        raise ValueError(f"unsupported target dtype {target_dtype!r}; expected one of {supported}")
    input_data = input_path.read_bytes()
    header, data_start = _load_safetensors_header(input_data)
    payload = bytearray()
    rewritten_header: dict[str, Any] = {}
    offset = 0
    converted_tensors = 0
    preserved_tensors = 0

    for name, record in header.items():
        if not _is_tensor_record(record):
            rewritten_header[name] = record
            continue
        src_dtype = record["dtype"]
        start, end = (int(record["data_offsets"][0]), int(record["data_offsets"][1]))
        raw = input_data[data_start + start : data_start + end]
        if src_dtype in _FLOAT_DTYPE_NAMES:
            out_raw = _convert_tensor_payload(raw, src_dtype, target_dtype)
            out_dtype = target_dtype
            if src_dtype != target_dtype:
                converted_tensors += 1
            else:
                preserved_tensors += 1
        else:
            out_raw = raw
            out_dtype = src_dtype
            preserved_tensors += 1
        rewritten_header[name] = {
            **record,
            "dtype": out_dtype,
            "data_offsets": [offset, offset + len(out_raw)],
        }
        payload.extend(out_raw)
        offset += len(out_raw)

    header_bytes = json.dumps(rewritten_header, separators=(",", ":")).encode("utf-8")
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with output_path.open("wb") as handle:
        handle.write(struct.pack("<Q", len(header_bytes)))
        handle.write(header_bytes)
        handle.write(payload)
    return ConversionStats(
        input_bytes=len(input_data),
        output_bytes=output_path.stat().st_size,
        converted_tensors=converted_tensors,
        preserved_tensors=preserved_tensors,
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Convert floating-point tensors in a safetensors file to a target dtype."
    )
    parser.add_argument("input", type=Path, help="Input .safetensors file")
    parser.add_argument("output", type=Path, help="Output .safetensors file")
    parser.add_argument(
        "--target-dtype",
        default="F16",
        choices=sorted(_TARGET_DTYPE_NAMES),
        help="Target dtype for floating-point tensor payloads (default: F16)",
    )
    args = parser.parse_args()
    stats = convert_safetensors_dtype(
        args.input,
        args.output,
        target_dtype=args.target_dtype,
    )
    print(
        json.dumps(
            {
                "input": str(args.input),
                "output": str(args.output),
                "target_dtype": args.target_dtype,
                "input_bytes": stats.input_bytes,
                "output_bytes": stats.output_bytes,
                "converted_tensors": stats.converted_tensors,
                "preserved_tensors": stats.preserved_tensors,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
