"""
molt.gpu — GPU compute support for Molt.

Usage:
    from molt import gpu

    @gpu.kernel
    def vector_add(a: gpu.Buffer[float], b: gpu.Buffer[float],
                   c: gpu.Buffer[float], n: int):
        tid = gpu.thread_id()
        if tid < n:
            c[tid] = a[tid] + b[tid]

    # Allocate and launch
    a_gpu = gpu.to_device(a_host)
    b_gpu = gpu.to_device(b_host)
    c_gpu = gpu.alloc(n, float)
    vector_add[256, 256](a_gpu, b_gpu, c_gpu, n)
    result = gpu.from_device(c_gpu)
"""

from __future__ import annotations

import struct
import array
import operator
from _intrinsics import require_intrinsic as _require_intrinsic
from _intrinsics import runtime_active as _runtime_active


def _default_format_char(element_type: type) -> str:
    return "d" if element_type is float else "q"


def _format_itemsize(format_char: str) -> int:
    return struct.calcsize(format_char)


def _launch_intrinsic():
    cached = globals().get("_MOLT_GPU_KERNEL_LAUNCH")
    if cached is None:
        cached = _require_intrinsic("molt_gpu_kernel_launch_python")
        globals()["_MOLT_GPU_KERNEL_LAUNCH"] = cached
    return cached


def _require_positive_launch_dim(value, field_name: str) -> int:
    if isinstance(value, bool):
        raise TypeError(f"GPU launch {field_name} must be a positive integer")
    try:
        number = operator.index(value)
    except (TypeError, ValueError) as exc:
        raise TypeError(f"GPU launch {field_name} must be a positive integer") from exc
    if number <= 0:
        raise ValueError(f"GPU launch {field_name} must be positive")
    return number


def _normalize_launch_config(config) -> tuple[int, int]:
    if isinstance(config, dict):
        keys = set(config)
        allowed = {"grid", "threads"}
        unknown = keys - allowed
        if unknown:
            names = ", ".join(sorted(str(key) for key in unknown))
            raise ValueError(f"unknown GPU launch config field(s): {names}")
        if not keys:
            raise ValueError("GPU launch config dict must set grid or threads")
        grid = _require_positive_launch_dim(config.get("grid", 256), "grid")
        threads = _require_positive_launch_dim(config.get("threads", 256), "threads")
        return grid, threads
    if isinstance(config, tuple):
        if len(config) == 1:
            grid = _require_positive_launch_dim(config[0], "grid")
            return grid, 256
        if len(config) == 2:
            grid = _require_positive_launch_dim(config[0], "grid")
            threads = _require_positive_launch_dim(config[1], "threads")
            return grid, threads
        raise ValueError("GPU launch tuple config must contain grid or grid, threads")
    grid = _require_positive_launch_dim(config, "grid")
    return grid, 256


class Buffer:
    """GPU buffer handle. Created via gpu.to_device() or gpu.alloc()."""

    def __class_getitem__(cls, _item):
        return cls

    def __init__(
        self,
        data: bytes | bytearray,
        element_type: type,
        size: int,
        *,
        format_char: str | None = None,
    ):
        self._data = data
        self._element_type = element_type
        self._size = size
        self._format_char = format_char or _default_format_char(element_type)
        self._itemsize = _format_itemsize(self._format_char)
        if len(self._data) < self._size * self._itemsize:
            raise ValueError(
                f"Buffer payload too small for {self._size} items of format {self._format_char}"
            )

    @property
    def nbytes(self) -> int:
        return len(self._data)

    @property
    def size(self) -> int:
        return self._size

    @property
    def element_type(self) -> type:
        return self._element_type

    @property
    def format_char(self) -> str:
        return self._format_char

    @property
    def itemsize(self) -> int:
        return self._itemsize

    def __getitem__(self, index: int):
        """Read element at index from the buffer."""
        if index < 0 or index >= self._size:
            raise IndexError(f"Buffer index {index} out of range [0, {self._size})")
        offset = index * self._itemsize
        return struct.unpack_from(self._format_char, self._data, offset)[0]

    def __setitem__(self, index: int, value):
        """Write element at index into the buffer."""
        if index < 0 or index >= self._size:
            raise IndexError(f"Buffer index {index} out of range [0, {self._size})")
        # Convert immutable bytes to bytearray if needed
        if isinstance(self._data, bytes):
            self._data = bytearray(self._data)
        offset = index * self._itemsize
        if self._element_type is float:
            struct.pack_into(self._format_char, self._data, offset, float(value))
        else:
            struct.pack_into(self._format_char, self._data, offset, int(value))


def to_device(data) -> Buffer:
    """Copy host data to a GPU buffer.

    Accepts: list[int], list[float], array.array, bytes, or any sequence.
    """
    if isinstance(data, bytes):
        return Buffer(data, int, len(data) // 8, format_char="q")
    elif isinstance(data, array.array):
        raw = data.tobytes()
        if data.typecode in ("f", "d"):
            return Buffer(raw, float, len(data), format_char=data.typecode)
        return Buffer(raw, int, len(data), format_char="q")
    elif isinstance(data, (list, tuple)):
        if not data:
            return Buffer(b"", float, 0)
        if isinstance(data[0], float):
            raw = struct.pack(f"{len(data)}d", *data)
            return Buffer(raw, float, len(data), format_char="d")
        else:
            raw = struct.pack(f"{len(data)}q", *data)
            return Buffer(raw, int, len(data), format_char="q")
    else:
        raise TypeError(f"Cannot convert {type(data)} to GPU buffer")


def from_device(buf: Buffer) -> list:
    """Copy GPU buffer back to host as a Python list."""
    count = buf.size
    if count == 0:
        return []
    width = buf.itemsize
    return list(struct.unpack(f"{count}{buf.format_char}", buf._data[: count * width]))


def alloc(size: int, dtype: type = float, *, format_char: str | None = None) -> Buffer:
    """Allocate an empty GPU buffer."""
    resolved_format = format_char or _default_format_char(dtype)
    elem_size = _format_itemsize(resolved_format)
    return Buffer(bytearray(size * elem_size), dtype, size, format_char=resolved_format)


# These are the five public query callables. A compiled runtime publishes its
# actual native functions, so kernel calls and hardware binding admission share
# the canonical intrinsic identity. Host reference functions exist only outside
# an active Molt runtime.
if _runtime_active():
    thread_id = _require_intrinsic("molt_gpu_thread_id")
    block_id = _require_intrinsic("molt_gpu_block_id")
    block_dim = _require_intrinsic("molt_gpu_block_dim")
    grid_dim = _require_intrinsic("molt_gpu_grid_dim")
    barrier = _require_intrinsic("molt_gpu_barrier")
else:

    def _reference_geometry():
        context = globals().get("_MOLT_GPU_REFERENCE_CONTEXT")
        geometry = None if context is None else context.get()
        return (0, 0, 1, 1) if geometry is None else geometry

    def thread_id() -> int:
        """Current logical thread in the development CPython reference lane."""
        return _reference_geometry()[0]

    def block_id() -> int:
        """Current reference workgroup."""
        return _reference_geometry()[1]

    def block_dim() -> int:
        """Current reference workgroup size."""
        return _reference_geometry()[2]

    def grid_dim() -> int:
        """Current reference grid size."""
        return _reference_geometry()[3]

    def barrier():
        """Reject a collective operation outside parallel hardware execution."""
        raise RuntimeError(
            "GPU barrier requires a parallel hardware kernel execution context"
        )

    def _reference_launch(func, grid: int, threads: int, args):
        # No ContextVar import or object is needed until a host reference launch.
        from contextvars import ContextVar

        context = globals().get("_MOLT_GPU_REFERENCE_CONTEXT")
        if context is None:
            context = globals().setdefault(
                "_MOLT_GPU_REFERENCE_CONTEXT",
                ContextVar("molt_gpu_geometry", default=None),
            )
        for tid in range(grid * threads):
            token = context.set((tid, tid // threads, threads, grid))
            try:
                func(*args)
            finally:
                context.reset(token)


class _KernelLauncher:
    """Wraps a GPU kernel function for launch configuration."""

    def __init__(self, func, *, grid: int = 256, threads: int = 256):
        self._func = func
        self._name = func.__name__
        self._grid = grid
        self._threads = threads

    def __getitem__(self, config):
        """Configure launch: kernel[grid, threads] or kernel[total_threads]"""
        grid, threads = _normalize_launch_config(config)
        return _KernelLauncher(self._func, grid=grid, threads=threads)

    def __call__(self, *args):
        """Launch the kernel with the given arguments.

        Molt uses its native executor and selected hardware backend. Development
        CPython uses a scoped sequential reference context. Both preserve kernel
        exceptions and require explicit bounds checks in the kernel body.
        """
        grid = _require_positive_launch_dim(self._grid, "grid")
        threads = _require_positive_launch_dim(self._threads, "threads")
        total_threads = grid * threads
        # Geometry queries and the native launch ABI use signed 64-bit indices.
        if total_threads > (1 << 63) - 1:
            raise OverflowError("GPU launch geometry exceeds signed 64-bit indices")
        if _runtime_active():
            return _launch_intrinsic()(self._func, grid, threads, args)
        return _reference_launch(self._func, grid, threads, args)


def kernel(func):
    """Decorator that marks a function as a GPU compute kernel.

    Usage:
        @gpu.kernel
        def my_kernel(a: gpu.Buffer[float], b: gpu.Buffer[float], n: int):
            tid = gpu.thread_id()
            if tid < n:
                b[tid] = a[tid] * 2.0

    Development CPython execution is sequential. Molt execution uses the native
    launch authority, which selects its admitted CPU or hardware backend.
    """
    return _KernelLauncher(func)
