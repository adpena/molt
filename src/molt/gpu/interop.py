"""
molt.gpu.interop — SafeTensors and JSON tensor loading helpers.

NumPy/NPZ helpers live in ``molt.gpu.numpy_io`` so callers that only need
SafeTensors do not pay the compile-time cost of unrelated format loaders.
"""

import json
import math
import struct
from .tensor import Tensor


# SafeTensors dtype mapping: name -> (struct format char, byte size)
_SAFETENSOR_DTYPES = {
    "F64": ("d", 8),
    "F32": ("f", 4),
    "F16": (None, 2),
    "BF16": (None, 2),
    "I64": ("q", 8),
    "I32": ("i", 4),
    "I16": ("h", 2),
    "I8": ("b", 1),
    "U8": ("B", 1),
    "BOOL": ("?", 1),
}


def _decode_f16(raw: bytes) -> list:
    """Decode IEEE 754 half-precision floats to Python floats."""
    result = []
    for i in range(0, len(raw), 2):
        h = struct.unpack_from("<H", raw, i)[0]
        sign = (h >> 15) & 1
        exp = (h >> 10) & 0x1F
        frac = h & 0x3FF

        if exp == 0:
            val = math.ldexp(frac, -24)
        elif exp == 31:
            val = float("inf") if frac == 0 else float("nan")
        else:
            val = math.ldexp(frac + 1024, exp - 25)

        if sign:
            val = -val
        result.append(val)
    return result


def _decode_bf16(raw: bytes) -> list:
    """Decode BFloat16 values to Python floats."""
    result = []
    for i in range(0, len(raw), 2):
        h = struct.unpack_from("<H", raw, i)[0]
        f32_bits = h << 16
        val = struct.unpack("<f", struct.pack("<I", f32_bits))[0]
        result.append(val)
    return result


def load_safetensors(path: str) -> dict:
    """Load weights from a .safetensors file."""
    with open(path, "rb") as f:
        data = f.read()

    header_len = struct.unpack_from("<Q", data, 0)[0]
    if header_len > len(data) - 8:
        raise ValueError("SafeTensors header length exceeds file size")
    header_json = data[8 : 8 + header_len].decode("utf-8")
    header = json.loads(header_json)

    data_start = 8 + header_len
    tensors = {}

    for name, meta in header.items():
        if name == "__metadata__":
            continue

        dtype_str = meta["dtype"]
        shape = tuple(meta["shape"])
        start, end = meta["data_offsets"]

        raw = data[data_start + start : data_start + end]

        if dtype_str == "F16":
            values = _decode_f16(raw)
        elif dtype_str == "BF16":
            values = _decode_bf16(raw)
        else:
            info = _SAFETENSOR_DTYPES.get(dtype_str)
            if info is None:
                raise ValueError(f"Unsupported SafeTensors dtype: {dtype_str}")
            fmt_char, elem_size = info
            count = len(raw) // elem_size
            values = list(struct.unpack(f"<{count}{fmt_char}", raw))

        values = [float(v) for v in values]
        tensors[name] = Tensor(values, shape=shape)

    return tensors


def load_json_weights(path: str) -> dict:
    """Load weights from a JSON file."""
    with open(path, "r") as f:
        data = json.load(f)

    tensors = {}
    for name, value in data.items():
        tensors[name] = Tensor(value)

    return tensors


def save_json_weights(tensors: dict, path: str):
    """Save tensors to a JSON file."""
    data = {}
    for name, tensor in tensors.items():
        data[name] = tensor.to_list()

    with open(path, "w") as f:
        json.dump(data, f)
