"""
molt.gpu.interop — SafeTensors and JSON tensor loading helpers.

NumPy/NPZ helpers live in ``molt.gpu.numpy_io`` so callers that only need
SafeTensors do not pay the compile-time cost of unrelated format loaders.
"""

import json
import math
import struct
import _intrinsics as _molt_intrinsics
from . import Buffer

Tensor = None

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


def _load_optional_intrinsic(name: str):
    loader = getattr(_molt_intrinsics, "load_intrinsic", None)
    if callable(loader):
        return loader(name)
    require = getattr(_molt_intrinsics, "require_intrinsic", None)
    if callable(require):
        try:
            return require(name)
        except RuntimeError:
            return None
    return None


_UNRESOLVED = object()
_MOLT_GPU_INTEROP_DECODE_F16_BYTES_TO_F32 = _UNRESOLVED
_MOLT_GPU_INTEROP_DECODE_BF16_BYTES_TO_F32 = _UNRESOLVED


def _resolve_optional_intrinsic(cache_name: str, intrinsic_name: str):
    intrinsic = globals().get(cache_name, _UNRESOLVED)
    if intrinsic is not _UNRESOLVED:
        return intrinsic

    loader = getattr(_molt_intrinsics, "load_intrinsic", None)
    if callable(loader):
        try:
            intrinsic = loader(intrinsic_name)
        except RuntimeError:
            intrinsic = None
        else:
            if intrinsic is not None:
                globals()[cache_name] = intrinsic
                return intrinsic

    require = getattr(_molt_intrinsics, "require_intrinsic", None)
    if callable(require):
        runtime_active = getattr(_molt_intrinsics, "runtime_active", None)
        try:
            intrinsic = require(intrinsic_name)
        except RuntimeError:
            if callable(runtime_active) and runtime_active():
                raise RuntimeError(f"intrinsic unavailable: {intrinsic_name}")
        else:
            if intrinsic is not None:
                globals()[cache_name] = intrinsic
                return intrinsic

    runtime_active = getattr(_molt_intrinsics, "runtime_active", None)
    if callable(runtime_active) and runtime_active():
        raise RuntimeError(f"intrinsic unavailable: {intrinsic_name}")
    return None


class _SafeTensorMap:
    """Lazy SafeTensors mapping that materializes tensors on first access."""

    def __init__(self, data: bytes, data_start: int, entries: dict):
        self._data = data
        self._data_start = data_start
        self._entries = entries
        self._cache = {}

    def __len__(self) -> int:
        return len(self._entries)

    def __iter__(self):
        return iter(self._entries)

    def __contains__(self, key) -> bool:
        return key in self._entries

    def __getitem__(self, key):
        if key in self._cache:
            return self._cache[key]
        meta = self._entries[key]
        tensor = _load_safetensor_entry(self._data, self._data_start, meta)
        self._cache[key] = tensor
        return tensor

    def get(self, key, default=None):
        if key not in self._entries:
            return default
        return self[key]

    def keys(self):
        return self._entries.keys()

    def items(self):
        for key in self._entries:
            yield key, self[key]

    def values(self):
        for key in self._entries:
            yield self[key]


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


def _decode_safetensor_values(raw: bytes, dtype_str: str) -> list:
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
    return [float(v) for v in values]


def _load_safetensor_entry(data: bytes, data_start: int, meta: dict):
    global Tensor
    if Tensor is None:
        from .tensor import Tensor as _Tensor

        Tensor = _Tensor

    dtype_str = meta["dtype"]
    shape = tuple(meta["shape"])
    start, end = meta["data_offsets"]
    raw = data[data_start + start : data_start + end]
    if dtype_str == "F64":
        count = len(raw) // 8
        return Tensor(Buffer(raw, float, count), shape=shape)
    if dtype_str == "F32":
        count = len(raw) // 4
        return Tensor(Buffer(raw, float, count, format_char="f"), shape=shape)
    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_INTEROP_DECODE_F16_BYTES_TO_F32",
        "molt_gpu_interop_decode_f16_bytes_to_f32",
    )
    if dtype_str == "F16" and callable(intrinsic):
        converted = intrinsic(raw)
        count = len(raw) // 2
        return Tensor(Buffer(converted, float, count, format_char="f"), shape=shape)
    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_INTEROP_DECODE_BF16_BYTES_TO_F32",
        "molt_gpu_interop_decode_bf16_bytes_to_f32",
    )
    if dtype_str == "BF16" and callable(intrinsic):
        converted = intrinsic(raw)
        count = len(raw) // 2
        return Tensor(Buffer(converted, float, count, format_char="f"), shape=shape)
    values = _decode_safetensor_values(raw, dtype_str)
    return Tensor(values, shape=shape)


def load_safetensors_bytes(data: bytes) -> _SafeTensorMap:
    """Load weights from an in-memory .safetensors blob."""
    header_len = struct.unpack_from("<Q", data, 0)[0]
    if header_len > len(data) - 8:
        raise ValueError("SafeTensors header length exceeds file size")
    header_json = data[8 : 8 + header_len].decode("utf-8")
    header = json.loads(header_json)

    data_start = 8 + header_len
    entries = {name: meta for name, meta in header.items() if name != "__metadata__"}
    return _SafeTensorMap(data, data_start, entries)


def load_safetensors(path: str) -> _SafeTensorMap:
    """Load weights from a .safetensors file."""
    with open(path, "rb") as f:
        return load_safetensors_bytes(f.read())


def load_json_weights(path: str) -> dict:
    """Load weights from a JSON file."""
    from .tensor import Tensor

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
