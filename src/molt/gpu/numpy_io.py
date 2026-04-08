"""
molt.gpu.numpy_io — NumPy/NPZ tensor loading helpers.

Split out of ``molt.gpu.interop`` so SafeTensors users do not compile the
NumPy/NPZ loader graph on the hot path.
"""

import struct
from .tensor import Tensor


_NUMPY_DTYPES = {
    "<f8": ("d", 8),
    "<f4": ("f", 4),
    ">f8": (">d", 8),
    ">f4": (">f", 4),
    "<i8": ("q", 8),
    "<i4": ("i", 4),
    "<i2": ("h", 2),
    "<i1": ("b", 1),
    "<u8": ("Q", 8),
    "<u4": ("I", 4),
    "<u2": ("H", 2),
    "<u1": ("B", 1),
    "|b1": ("?", 1),
    "|u1": ("B", 1),
    "|i1": ("b", 1),
}


def _parse_npy_header(data: bytes, offset: int = 0):
    magic = data[offset : offset + 6]
    if magic != b"\x93NUMPY":
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

    header_str = data[header_offset : header_offset + header_len].decode("latin-1").strip()
    data_offset = header_offset + header_len
    header_dict = _parse_numpy_header_dict(header_str)

    dtype_str = header_dict["descr"]
    fortran_order = header_dict.get("fortran_order", False)
    shape = header_dict["shape"]

    return dtype_str, shape, fortran_order, data_offset


def _parse_numpy_header_dict(s: str) -> dict:
    s = s.strip()
    if s.startswith("{") and s.endswith("}"):
        s = s[1:-1].strip()
        if s.endswith(","):
            s = s[:-1].strip()

    result = {}
    parts = []
    depth = 0
    current = []
    for ch in s:
        if ch == "(" or ch == "[":
            depth += 1
            current.append(ch)
        elif ch == ")" or ch == "]":
            depth -= 1
            current.append(ch)
        elif ch == "," and depth == 0:
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
        val_str = part[colon_idx + 1 :].strip()

        if val_str.startswith("'") or val_str.startswith('"'):
            result[key] = val_str.strip("'\"")
        elif val_str == "True":
            result[key] = True
        elif val_str == "False":
            result[key] = False
        elif val_str.startswith("("):
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

    raw = data[data_offset : data_offset + total_elems * elem_size]
    if dtype_str.startswith(">"):
        values = list(struct.unpack(f">{total_elems}{fmt_char[-1]}", raw))
    else:
        values = list(struct.unpack(f"<{total_elems}{fmt_char}", raw))

    values = [float(v) for v in values]
    return Tensor(values, shape=shape)


def load_npz(path: str) -> dict:
    import zipfile

    tensors = {}
    with zipfile.ZipFile(path, "r") as zf:
        for name in zf.namelist():
            if not name.endswith(".npy"):
                continue
            tensor_name = name[:-4]
            npy_data = zf.read(name)

            dtype_str, shape, fortran_order, data_offset = _parse_npy_header(npy_data)
            if fortran_order:
                continue

            info = _NUMPY_DTYPES.get(dtype_str)
            if info is None:
                continue

            fmt_char, elem_size = info
            total_elems = 1
            for s in shape:
                total_elems *= s

            raw = npy_data[data_offset : data_offset + total_elems * elem_size]
            if dtype_str.startswith(">"):
                values = list(struct.unpack(f">{total_elems}{fmt_char[-1]}", raw))
            else:
                values = list(struct.unpack(f"<{total_elems}{fmt_char}", raw))

            values = [float(v) for v in values]
            tensors[tensor_name] = Tensor(values, shape=shape)

    return tensors


def from_numpy_array(arr) -> Tensor:
    shape = tuple(arr.shape)
    flat = arr.flatten().tolist()
    values = [float(v) for v in flat]
    return Tensor(values, shape=shape)


def to_numpy_array(tensor: Tensor):
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
