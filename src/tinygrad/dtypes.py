"""tinygrad dtype descriptors backed by Molt scalar storage."""


class DType:
    """A tinygrad-compatible data type descriptor."""

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
    bool = DType("bool", 1, "?", 0)
    bool_ = bool
    int8 = DType("int8", 1, "b", 1)
    int16 = DType("int16", 2, "h", 2)
    int32 = DType("int32", 4, "i", 3)
    int64 = DType("int64", 8, "q", 4)
    uint8 = DType("uint8", 1, "B", 5)
    uint16 = DType("uint16", 2, "H", 6)
    uint32 = DType("uint32", 4, "I", 7)
    uint64 = DType("uint64", 8, "Q", 8)
    float16 = DType("float16", 2, "e", 9)
    bfloat16 = DType("bfloat16", 2, "e", 10)
    float32 = DType("float32", 4, "f", 11)
    float64 = DType("float64", 8, "d", 12)

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

    default_float = float32
    default_int = int32

    @staticmethod
    def from_py(t: type) -> DType:
        if t is _builtins_float:
            return dtypes.float32
        if t is _builtins_int:
            return dtypes.int32
        if t is _builtins_bool:
            return dtypes.bool
        raise TypeError(f"Cannot convert {t} to DType")


_builtins_float = float
_builtins_int = int
_builtins_bool = bool

dtypes = _DTypes()

bool = dtypes.bool
bool_ = dtypes.bool_
int8 = dtypes.int8
int16 = dtypes.int16
int32 = dtypes.int32
int64 = dtypes.int64
uint8 = dtypes.uint8
uint16 = dtypes.uint16
uint32 = dtypes.uint32
uint64 = dtypes.uint64
float16 = dtypes.float16
bfloat16 = dtypes.bfloat16
float32 = dtypes.float32
float64 = dtypes.float64
half = dtypes.half
float = dtypes.float
double = dtypes.double
short = dtypes.short
int = dtypes.int
long = dtypes.long
uchar = dtypes.uchar
ushort = dtypes.ushort
uint = dtypes.uint
ulong = dtypes.ulong

__all__ = [
    "DType",
    "dtypes",
    "bool",
    "bool_",
    "int8",
    "int16",
    "int32",
    "int64",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "float16",
    "bfloat16",
    "float32",
    "float64",
    "half",
    "float",
    "double",
    "short",
    "int",
    "long",
    "uchar",
    "ushort",
    "uint",
    "ulong",
]
