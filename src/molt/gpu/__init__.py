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
import _intrinsics as _molt_intrinsics


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


_MOLT_GPU_KERNEL_LAUNCH = _load_optional_intrinsic("molt_gpu_kernel_launch")


def _default_format_char(element_type: type) -> str:
    return "d" if element_type == float else "q"


def _format_itemsize(format_char: str) -> int:
    return struct.calcsize(format_char)


class Buffer:
    """GPU buffer handle. Created via gpu.to_device() or gpu.alloc()."""

    def __class_getitem__(cls, _item):
        return cls

    def __init__(
        self,
        data: bytes,
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
        if self._element_type == float:
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
            return Buffer(b'', float, 0)
        if isinstance(data[0], float):
            raw = struct.pack(f'{len(data)}d', *data)
            return Buffer(raw, float, len(data), format_char="d")
        else:
            raw = struct.pack(f'{len(data)}q', *data)
            return Buffer(raw, int, len(data), format_char="q")
    else:
        raise TypeError(f"Cannot convert {type(data)} to GPU buffer")


def from_device(buf: Buffer) -> list:
    """Copy GPU buffer back to host as a Python list."""
    count = buf.size
    if count == 0:
        return []
    width = buf.itemsize
    return list(struct.unpack(f'{count}{buf.format_char}', buf._data[:count * width]))

def alloc(size: int, dtype: type = float, *, format_char: str | None = None) -> Buffer:
    """Allocate an empty GPU buffer."""
    resolved_format = format_char or _default_format_char(dtype)
    elem_size = _format_itemsize(resolved_format)
    return Buffer(bytearray(size * elem_size), dtype, size, format_char=resolved_format)


def thread_id() -> int:
    """Get the current GPU thread ID.

    This is the logical GPU thread ID primitive. When Molt has a real compiled
    GPU-kernel lowering path active, it maps to:
    - Metal: [[thread_position_in_grid]]
    - WGSL: @builtin(global_invocation_id).x
    - CUDA: blockIdx.x * blockDim.x + threadIdx.x
    - HIP: hipBlockIdx_x * hipBlockDim_x + hipThreadIdx_x

    Until that lowering path is active, interpreted and compiled sequential
    launcher fallback both treat it as a normal Python function so the launcher
    can override it at runtime for correctness.
    """
    return 0


def block_id() -> int:
    """Get the current GPU block/workgroup ID. Compile-time intrinsic."""
    return 0


def block_dim() -> int:
    """Get the GPU block/workgroup size. Compile-time intrinsic."""
    return 1


def grid_dim() -> int:
    """Get the GPU grid dimension. Compile-time intrinsic."""
    return 1


def barrier():
    """GPU threadgroup synchronization barrier. Compile-time intrinsic."""
    pass


class _KernelLauncher:
    """Wraps a GPU kernel function for launch configuration."""

    def __init__(self, func):
        self._func = func
        self._name = func.__name__
        self._grid = None
        self._threads = None

    def __getitem__(self, config):
        """Configure launch: kernel[grid, threads] or kernel[total_threads]"""
        if isinstance(config, dict):
            self._grid = config.get('grid', 256)
            self._threads = config.get('threads', 256)
        elif isinstance(config, tuple):
            if len(config) >= 2:
                self._grid = config[0]
                self._threads = config[1]
            elif len(config) == 1:
                self._grid = config[0]
                self._threads = 256
        elif isinstance(config, int):
            self._grid = config
            self._threads = 256
        return self

    def __call__(self, *args):
        """Launch the kernel with the given arguments.

        In interpreted mode, and in compiled lanes that have not yet routed the
        kernel through a real GPU backend, this runs the kernel function
        sequentially for each logical thread ID. When a real compiled GPU
        lowering path is active, the compiler/runtime may replace this with
        backend dispatch via the GPU pipeline.
        """
        grid = self._grid or 256
        threads = self._threads or 256
        if callable(_MOLT_GPU_KERNEL_LAUNCH):
            return _MOLT_GPU_KERNEL_LAUNCH(self._func, grid, threads, args)
        total_threads = grid * threads if isinstance(grid, int) else grid

        # Interpreted fallback: simulate GPU execution sequentially
        import molt.gpu as gpu_module
        original_thread_id = gpu_module.thread_id

        for tid in range(total_threads):
            # Monkey-patch thread_id to return current tid
            gpu_module.thread_id = lambda _tid=tid: _tid
            try:
                self._func(*args)
            except IndexError:
                pass  # Thread ID out of bounds — expected for guard patterns

        # Restore
        gpu_module.thread_id = original_thread_id


def kernel(func):
    """Decorator that marks a function as a GPU compute kernel.

    Usage:
        @gpu.kernel
        def my_kernel(a: gpu.Buffer[float], b: gpu.Buffer[float], n: int):
            tid = gpu.thread_id()
            if tid < n:
                b[tid] = a[tid] * 2.0

    This marks the function as GPU-kernel-shaped. Interpreted execution runs
    sequentially. Compiled execution must preserve that sequential fallback
    until a real backend dispatch path is active for the target/backend lane.
    """
    return _KernelLauncher(func)
