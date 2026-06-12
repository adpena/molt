from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")

"""
tinygrad.dtypes — Data type descriptors.

Maps 1:1 to molt_gpu::dtype::DType and tinygrad's dtypes module.
"""


class DType:
    """A data type descriptor."""

    __slots__ = ("name", "itemsize", "fmt", "code")

    def __init__(self, name: str, itemsize: int, fmt: str, code: int | None = None) -> None:
        self.name = name
        self.itemsize = itemsize
        self.fmt = fmt
        self.code = code

    def __repr__(self) -> str:
        return f"dtypes.{self.name}"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, DType):
            return NotImplemented
        return self.name == other.name

    def __hash__(self) -> int:
        return hash(self.name)


class _DTypes:
    """Namespace for all dtype constants. Accessed as `dtypes.float32`, etc."""

    bool_ = DType("bool", 1, "?", 0)
    int8 = DType("int8", 1, "b", 1)
    int16 = DType("int16", 2, "h", 2)
    int32 = DType("int32", 4, "i", 3)
    int64 = DType("int64", 8, "q", 4)
    uint8 = DType("uint8", 1, "B", 5)
    uint16 = DType("uint16", 2, "H", 6)
    uint32 = DType("uint32", 4, "I", 7)
    uint64 = DType("uint64", 8, "Q", 8)
    float16 = DType("float16", 2, "e", 9)
    bfloat16 = DType(
        "bfloat16", 2, "e", 10
    )  # uses f16 struct format, bf16 handled at buffer level
    float32 = DType("float32", 4, "f", 11)
    float64 = DType("float64", 8, "d", 12)

    # Aliases matching tinygrad
    half = float16
    float = float32
    double = float64
    short = int16
    int = int32
    long = int64
    uchar = uint8
    ushort = uint16
    uint = uint32
    ulong = uint64

    # Default float type
    default_float = float32
    default_int = int32

    @staticmethod
    def from_py(t: type) -> "DType":
        """Convert a Python type to a DType."""
        if t is builtins_float:
            return dtypes.float32
        if t is builtins_int:
            return dtypes.int32
        if t is builtins_bool:
            return dtypes.bool_
        raise TypeError(f"Cannot convert {t} to DType")


# Avoid importing builtins at module level to prevent circular imports
builtins_float = float
builtins_int = int
builtins_bool = bool

dtypes = _DTypes()
