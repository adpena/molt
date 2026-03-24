"""
molt.gpu.interop — Load ML models from popular formats.

Supports:
- SafeTensors (.safetensors) — Hugging Face standard
- NumPy (.npy/.npz) — Scientific Python standard
- JSON weights — Simple key-to-list format

All parsers are pure Python (stdlib only, no external dependencies).
"""

import json
import math
import struct
import zipfile
from .tensor import Tensor


# ── SafeTensors ───────────────────────────────────────────────────────

# SafeTensors dtype mapping: name -> (struct format char, byte size)
_SAFETENSOR_DTYPES = {
    "F64": ("d", 8),
    "F32": ("f", 4),
    "F16": (None, 2),   # Half-float, decoded manually
    "BF16": (None, 2),  # BFloat16, decoded manually
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
            # Subnormal or zero
            val = math.ldexp(frac, -24)
        elif exp == 31:
            # Inf or NaN
            val = float('inf') if frac == 0 else float('nan')
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
        # BF16 is the upper 16 bits of a float32
        f32_bits = h << 16
        val = struct.unpack("<f", struct.pack("<I", f32_bits))[0]
        result.append(val)
    return result


def load_safetensors(path: str) -> dict:
    """Load weights from a .safetensors file.

    Returns dict of {name: Tensor}.

    SafeTensors binary format:
        - 8 bytes: header length (little-endian u64)
        - header_length bytes: JSON metadata header
        - remaining bytes: raw tensor data

    The JSON header maps tensor names to:
        {"dtype": "F32", "shape": [768, 768], "data_offsets": [start, end]}
    """
    with open(path, "rb") as f:
        data = f.read()

    # Parse header length
    header_len = struct.unpack_from("<Q", data, 0)[0]
    header_json = data[8:8 + header_len].decode("utf-8")
    header = json.loads(header_json)

    data_start = 8 + header_len
    tensors = {}

    for name, meta in header.items():
        if name == "__metadata__":
            continue  # Skip metadata entry

        dtype_str = meta["dtype"]
        shape = tuple(meta["shape"])
        start, end = meta["data_offsets"]

        raw = data[data_start + start:data_start + end]

        # Decode based on dtype
        if dtype_str in ("F16",):
            values = _decode_f16(raw)
        elif dtype_str in ("BF16",):
            values = _decode_bf16(raw)
        else:
            info = _SAFETENSOR_DTYPES.get(dtype_str)
            if info is None:
                raise ValueError(f"Unsupported SafeTensors dtype: {dtype_str}")
            fmt_char, elem_size = info
            count = len(raw) // elem_size
            values = list(struct.unpack(f"<{count}{fmt_char}", raw))

        # Convert all values to float for Tensor
        values = [float(v) for v in values]
        tensors[name] = Tensor(values, shape=shape)

    return tensors


# ── NumPy .npy ────────────────────────────────────────────────────────

# NumPy dtype mapping: descr string -> (struct format, byte size)
_NUMPY_DTYPES = {
    "<f8": ("d", 8),   # float64
    "<f4": ("f", 4),   # float32
    ">f8": (">d", 8),
    ">f4": (">f", 4),
    "<i8": ("q", 8),   # int64
    "<i4": ("i", 4),   # int32
    "<i2": ("h", 2),   # int16
    "<i1": ("b", 1),   # int8
    "<u8": ("Q", 8),   # uint64
    "<u4": ("I", 4),   # uint32
    "<u2": ("H", 2),   # uint16
    "<u1": ("B", 1),   # uint8
    "|b1": ("?", 1),   # bool
    "|u1": ("B", 1),   # uint8 (no endian)
    "|i1": ("b", 1),   # int8 (no endian)
}


def _parse_npy_header(data: bytes, offset: int = 0):
    """Parse a .npy file header, returning (dtype_str, shape, fortran_order, data_offset)."""
    # Magic: \x93NUMPY
    magic = data[offset:offset + 6]
    if magic != b'\x93NUMPY':
        raise ValueError("Not a valid .npy file (bad magic)")

    major = data[offset + 6]
    minor = data[offset + 7]

    if major == 1:
        header_len = struct.unpack_from("<H", data, offset + 8)[0]
        header_offset = offset + 10
    elif major == 2:
        header_len = struct.unpack_from("<I", data, offset + 8)[0]
        header_offset = offset + 12
    else:
        raise ValueError(f"Unsupported .npy version: {major}.{minor}")

    header_str = data[header_offset:header_offset + header_len].decode("latin-1").strip()
    data_offset = header_offset + header_len

    # Parse the header dict string (a restricted Python literal)
    header_dict = _parse_numpy_header_dict(header_str)

    dtype_str = header_dict["descr"]
    fortran_order = header_dict.get("fortran_order", False)
    shape = header_dict["shape"]

    return dtype_str, shape, fortran_order, data_offset


def _parse_numpy_header_dict(s: str) -> dict:
    """Parse a numpy header dictionary string.

    This is a restricted Python literal; we handle the common cases
    with manual parsing for safety (no dynamic code execution).
    """
    s = s.strip()
    if s.startswith("{") and s.endswith("}"):
        s = s[1:-1].strip()
        if s.endswith(","):
            s = s[:-1].strip()

    result = {}
    # Split by top-level commas (not inside parens)
    parts = []
    depth = 0
    current = []
    for ch in s:
        if ch == '(' or ch == '[':
            depth += 1
            current.append(ch)
        elif ch == ')' or ch == ']':
            depth -= 1
            current.append(ch)
        elif ch == ',' and depth == 0:
            parts.append("".join(current).strip())
            current = []
        else:
            current.append(ch)
    if current:
        parts.append("".join(current).strip())

    for part in parts:
        if ":" not in part:
            continue
        colon_idx = part.index(":")
        key = part[:colon_idx].strip().strip("'\"")
        val_str = part[colon_idx + 1:].strip()

        if val_str.startswith("'") or val_str.startswith('"'):
            result[key] = val_str.strip("'\"")
        elif val_str == "True":
            result[key] = True
        elif val_str == "False":
            result[key] = False
        elif val_str.startswith("("):
            # Parse tuple of ints
            inner = val_str.strip("()")
            if inner.strip() == "":
                result[key] = ()
            else:
                nums = [int(x.strip()) for x in inner.split(",") if x.strip()]
                result[key] = tuple(nums)
        else:
            try:
                result[key] = int(val_str)
            except ValueError:
                result[key] = val_str

    return result


def load_numpy(path: str) -> Tensor:
    """Load a single tensor from a .npy file.

    Supports: float32, float64, int8-int64, uint8-uint64, bool.
    """
    with open(path, "rb") as f:
        data = f.read()

    dtype_str, shape, fortran_order, data_offset = _parse_npy_header(data)

    if fortran_order:
        raise ValueError("Fortran-order .npy files are not supported")

    info = _NUMPY_DTYPES.get(dtype_str)
    if info is None:
        raise ValueError(f"Unsupported numpy dtype: {dtype_str}")

    fmt_char, elem_size = info
    total_elems = 1
    for s in shape:
        total_elems *= s

    raw = data[data_offset:data_offset + total_elems * elem_size]

    # Handle endianness in format string
    if dtype_str.startswith(">"):
        values = list(struct.unpack(f">{total_elems}{fmt_char[-1]}", raw))
    else:
        values = list(struct.unpack(f"<{total_elems}{fmt_char}", raw))

    values = [float(v) for v in values]
    return Tensor(values, shape=shape)


def load_npz(path: str) -> dict:
    """Load multiple tensors from a .npz (compressed numpy) file.

    Returns dict of {name: Tensor}.
    """
    tensors = {}
    with zipfile.ZipFile(path, "r") as zf:
        for name in zf.namelist():
            if not name.endswith(".npy"):
                continue
            tensor_name = name[:-4]  # Strip .npy extension
            npy_data = zf.read(name)

            dtype_str, shape, fortran_order, data_offset = _parse_npy_header(npy_data)

            if fortran_order:
                continue  # Skip unsupported

            info = _NUMPY_DTYPES.get(dtype_str)
            if info is None:
                continue  # Skip unsupported dtype

            fmt_char, elem_size = info
            total_elems = 1
            for s in shape:
                total_elems *= s

            raw = npy_data[data_offset:data_offset + total_elems * elem_size]

            if dtype_str.startswith(">"):
                values = list(struct.unpack(f">{total_elems}{fmt_char[-1]}", raw))
            else:
                values = list(struct.unpack(f"<{total_elems}{fmt_char}", raw))

            values = [float(v) for v in values]
            tensors[tensor_name] = Tensor(values, shape=shape)

    return tensors


# ── JSON weights ──────────────────────────────────────────────────────

def load_json_weights(path: str) -> dict:
    """Load weights from a JSON file.

    Expected format:
        {
            "layer.weight": [[0.1, 0.2, ...], [0.3, 0.4, ...]],
            "layer.bias": [0.1, 0.2, ...]
        }

    Returns dict of {name: Tensor}.
    """
    with open(path, "r") as f:
        data = json.load(f)

    tensors = {}
    for name, value in data.items():
        tensors[name] = Tensor(value)

    return tensors


def save_json_weights(tensors: dict, path: str):
    """Save tensors to a JSON file.

    Args:
        tensors: dict of {name: Tensor}
        path: output file path
    """
    data = {}
    for name, tensor in tensors.items():
        data[name] = tensor.to_list()

    with open(path, "w") as f:
        json.dump(data, f)


# ── NumPy interop (when numpy is available) ───────────────────────────

def from_numpy_array(arr) -> Tensor:
    """Convert a numpy ndarray to a Tensor.

    Args:
        arr: numpy.ndarray

    Returns:
        Tensor with the same data and shape
    """
    shape = tuple(arr.shape)
    flat = arr.flatten().tolist()
    values = [float(v) for v in flat]
    return Tensor(values, shape=shape)


def to_numpy_array(tensor: Tensor):
    """Convert a Tensor to a numpy ndarray.

    Requires numpy to be importable.

    Returns:
        numpy.ndarray with the same data and shape
    """
    try:
        import numpy as np
    except ImportError:
        raise ImportError(
            "numpy is required for to_numpy_array(). "
            "Install it with: pip install numpy"
        )

    flat = tensor._data_list()
    arr = np.array(flat, dtype=np.float64)
    return arr.reshape(tensor.shape)
