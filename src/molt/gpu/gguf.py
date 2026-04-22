"""
molt.gpu.gguf — Parse GGUF files and expose metadata/tensors.

GGUF is the standard container format used by llama.cpp and related tooling.
This module parses metadata and tensor data from .gguf files. It does not
execute models or provide OCR/runtime inference support.
"""

import struct
from .tensor import Tensor

GGUF_MAGIC = 0x46475547  # "GGUF" in little-endian

# GGUF type enum
GGUF_TYPE_F32 = 0
GGUF_TYPE_F16 = 1
GGUF_TYPE_Q4_0 = 2
GGUF_TYPE_Q4_1 = 3
GGUF_TYPE_Q5_0 = 6
GGUF_TYPE_Q5_1 = 7
GGUF_TYPE_Q8_0 = 8
GGUF_TYPE_Q8_1 = 9

# Metadata value types
GGUF_META_UINT8 = 0
GGUF_META_INT8 = 1
GGUF_META_UINT16 = 2
GGUF_META_INT16 = 3
GGUF_META_UINT32 = 4
GGUF_META_INT32 = 5
GGUF_META_FLOAT32 = 6
GGUF_META_BOOL = 7
GGUF_META_STRING = 8
GGUF_META_ARRAY = 9
GGUF_META_UINT64 = 10
GGUF_META_INT64 = 11
GGUF_META_FLOAT64 = 12


class GGUFModel:
    """Parsed GGUF model file."""

    def __init__(self):
        self.metadata = {}
        self.tensors = {}  # name -> (shape, type, data_bytes)
        self.architecture = ""
        self.vocab_size = 0
        self.embed_dim = 0
        self.num_heads = 0
        self.num_layers = 0

    def get_tensor(self, name: str) -> Tensor:
        """Get a tensor by name, dequantized to float."""
        if name not in self.tensors:
            raise KeyError(f"Tensor '{name}' not found in GGUF model")
        shape, dtype, data = self.tensors[name]

        if dtype == GGUF_TYPE_F32:
            n = len(data) // 4
            values = list(struct.unpack(f"<{n}f", data))
            return Tensor(values, shape=tuple(shape))
        elif dtype == GGUF_TYPE_F16:
            n = len(data) // 2
            values = [
                _f16_to_f32(struct.unpack_from("<H", data, i * 2)[0]) for i in range(n)
            ]
            return Tensor(values, shape=tuple(shape))
        elif dtype in (GGUF_TYPE_Q8_0, GGUF_TYPE_Q8_1):
            # Q8 block dequantization
            values = _dequantize_q8(data, dtype)
            return Tensor(values[: _prod(shape)], shape=tuple(shape))
        elif dtype in (GGUF_TYPE_Q4_0, GGUF_TYPE_Q4_1):
            values = _dequantize_q4(data, dtype)
            return Tensor(values[: _prod(shape)], shape=tuple(shape))
        else:
            raise ValueError(f"Unsupported GGUF tensor type: {dtype}")


def load_gguf(path: str) -> GGUFModel:
    """Load a GGUF model file.

    Returns a GGUFModel with metadata and tensors accessible by name.
    """
    model = GGUFModel()

    with open(path, "rb") as f:
        # Header
        magic = struct.unpack("<I", f.read(4))[0]
        if magic != GGUF_MAGIC:
            raise ValueError(
                f"Not a GGUF file (magic: {magic:#x}, expected {GGUF_MAGIC:#x})"
            )

        version = struct.unpack("<I", f.read(4))[0]
        if version not in (2, 3):
            raise ValueError(f"Unsupported GGUF version: {version}")

        n_tensors = struct.unpack("<Q", f.read(8))[0]
        n_metadata = struct.unpack("<Q", f.read(8))[0]

        if n_metadata > 100_000:
            raise ValueError("GGUF metadata count too large")
        if n_tensors > 100_000:
            raise ValueError("GGUF tensor count too large")

        # Read metadata key-value pairs
        for _ in range(n_metadata):
            key = _read_string(f, version)
            value = _read_meta_value(f, version)
            model.metadata[key] = value

        # Extract common metadata
        model.architecture = model.metadata.get("general.architecture", "")
        arch = model.architecture
        model.vocab_size = model.metadata.get(f"{arch}.vocab_size", 0)
        model.embed_dim = model.metadata.get(f"{arch}.embedding_length", 0)
        model.num_heads = model.metadata.get(f"{arch}.attention.head_count", 0)
        model.num_layers = model.metadata.get(f"{arch}.block_count", 0)

        # Read tensor metadata
        tensor_infos = []
        for _ in range(n_tensors):
            name = _read_string(f, version)
            n_dims = struct.unpack("<I", f.read(4))[0]
            dims = [struct.unpack("<Q", f.read(8))[0] for _ in range(n_dims)]
            dtype = struct.unpack("<I", f.read(4))[0]
            offset = struct.unpack("<Q", f.read(8))[0]
            tensor_infos.append((name, dims, dtype, offset))

        # Align to 32 bytes for tensor data
        pos = f.tell()
        alignment = 32
        padding = (alignment - (pos % alignment)) % alignment
        f.seek(pos + padding)
        data_start = f.tell()

        # Read tensor data
        for name, dims, dtype, offset in tensor_infos:
            n_elements = _prod(dims) if dims else 1
            byte_size = _tensor_byte_size(n_elements, dtype)
            f.seek(data_start + offset)
            data = f.read(byte_size)
            model.tensors[name] = (dims, dtype, data)

    return model


# --- Helpers ---


def _read_string(f, version):
    length = struct.unpack("<Q", f.read(8))[0]
    if length > 10_000_000:
        raise ValueError("GGUF string too long")
    return f.read(length).decode("utf-8")


def _read_meta_value(f, version):
    vtype = struct.unpack("<I", f.read(4))[0]
    if vtype == GGUF_META_STRING:
        return _read_string(f, version)
    elif vtype == GGUF_META_UINT32:
        return struct.unpack("<I", f.read(4))[0]
    elif vtype == GGUF_META_INT32:
        return struct.unpack("<i", f.read(4))[0]
    elif vtype == GGUF_META_UINT64:
        return struct.unpack("<Q", f.read(8))[0]
    elif vtype == GGUF_META_INT64:
        return struct.unpack("<q", f.read(8))[0]
    elif vtype == GGUF_META_FLOAT32:
        return struct.unpack("<f", f.read(4))[0]
    elif vtype == GGUF_META_FLOAT64:
        return struct.unpack("<d", f.read(8))[0]
    elif vtype == GGUF_META_BOOL:
        return struct.unpack("<?", f.read(1))[0]
    elif vtype == GGUF_META_UINT16:
        return struct.unpack("<H", f.read(2))[0]
    elif vtype == GGUF_META_INT16:
        return struct.unpack("<h", f.read(2))[0]
    elif vtype == GGUF_META_UINT8:
        return struct.unpack("<B", f.read(1))[0]
    elif vtype == GGUF_META_INT8:
        return struct.unpack("<b", f.read(1))[0]
    elif vtype == GGUF_META_ARRAY:
        elem_type = struct.unpack("<I", f.read(4))[0]
        length = struct.unpack("<Q", f.read(8))[0]
        return [_read_meta_value_by_type(f, elem_type, version) for _ in range(length)]
    else:
        raise ValueError(f"Unsupported GGUF metadata type: {vtype}")


def _read_meta_value_by_type(f, vtype, version):
    if vtype == GGUF_META_STRING:
        return _read_string(f, version)
    elif vtype in (GGUF_META_UINT32, GGUF_META_INT32):
        return struct.unpack("<I" if vtype == GGUF_META_UINT32 else "<i", f.read(4))[0]
    elif vtype == GGUF_META_UINT64:
        return struct.unpack("<Q", f.read(8))[0]
    elif vtype == GGUF_META_INT64:
        return struct.unpack("<q", f.read(8))[0]
    elif vtype == GGUF_META_FLOAT64:
        return struct.unpack("<d", f.read(8))[0]
    elif vtype == GGUF_META_UINT16:
        return struct.unpack("<H", f.read(2))[0]
    elif vtype == GGUF_META_INT16:
        return struct.unpack("<h", f.read(2))[0]
    elif vtype == GGUF_META_UINT8:
        return struct.unpack("<B", f.read(1))[0]
    elif vtype == GGUF_META_INT8:
        return struct.unpack("<b", f.read(1))[0]
    elif vtype == GGUF_META_FLOAT32:
        return struct.unpack("<f", f.read(4))[0]
    elif vtype == GGUF_META_BOOL:
        return struct.unpack("<?", f.read(1))[0]
    elif vtype == GGUF_META_ARRAY:
        raise ValueError("Nested GGUF arrays are not supported")
    else:
        raise ValueError(f"Unsupported GGUF metadata type: {vtype}")


def _f16_to_f32(h):
    sign = (h >> 15) & 1
    exp = (h >> 10) & 0x1F
    frac = h & 0x3FF
    if exp == 0:
        return (-1) ** sign * 2 ** (-14) * (frac / 1024)
    elif exp == 31:
        return float("-inf") if sign else float("inf") if frac == 0 else float("nan")
    else:
        return (-1) ** sign * 2 ** (exp - 15) * (1 + frac / 1024)


def _dequantize_q8(data, dtype):
    # Q8_0: block_size=32, each block = 1 f16 scale + 32 int8 values
    block_size = 32
    header_size = 2 + block_size  # 2 bytes f16 scale + 32 bytes int8
    n_blocks = len(data) // header_size
    values = []
    for i in range(n_blocks):
        offset = i * header_size
        scale = _f16_to_f32(struct.unpack_from("<H", data, offset)[0])
        for j in range(block_size):
            q = struct.unpack_from("b", data, offset + 2 + j)[0]
            values.append(q * scale)
    return values


def _dequantize_q4(data, dtype):
    # Q4_0: block_size=32, each block = 1 f16 scale + 16 bytes (32 nibbles)
    block_size = 32
    header_size = 2 + block_size // 2  # 2 bytes f16 scale + 16 bytes nibbles
    n_blocks = len(data) // header_size
    values = []
    for i in range(n_blocks):
        offset = i * header_size
        scale = _f16_to_f32(struct.unpack_from("<H", data, offset)[0])
        for j in range(block_size // 2):
            byte = data[offset + 2 + j]
            lo = (byte & 0x0F) - 8
            hi = ((byte >> 4) & 0x0F) - 8
            values.append(lo * scale)
            values.append(hi * scale)
    return values


def _tensor_byte_size(n_elements, dtype):
    if dtype == GGUF_TYPE_F32:
        return n_elements * 4
    elif dtype == GGUF_TYPE_F16:
        return n_elements * 2
    elif dtype in (GGUF_TYPE_Q8_0, GGUF_TYPE_Q8_1):
        n_blocks = (n_elements + 31) // 32
        return n_blocks * 34  # 2 + 32
    elif dtype in (GGUF_TYPE_Q4_0, GGUF_TYPE_Q4_1):
        n_blocks = (n_elements + 31) // 32
        return n_blocks * 18  # 2 + 16
    else:
        raise ValueError(f"Unsupported GGUF tensor type: {dtype}")


def _prod(lst):
    r = 1
    for x in lst:
        r *= x
    return r
