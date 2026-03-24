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

from typing import TypeVar, Generic, List, Optional
import struct
import array

T = TypeVar('T')


class Buffer(Generic[T]):
    """GPU buffer handle. Created via gpu.to_device() or gpu.alloc()."""

    def __init__(self, data: bytes, element_type: type, size: int):
        self._data = data
        self._element_type = element_type
        self._size = size

    @property
    def nbytes(self) -> int:
        return len(self._data)

    @property
    def size(self) -> int:
        return self._size

    @property
    def element_type(self) -> type:
        return self._element_type

    def __getitem__(self, index: int):
        """Read element at index from the buffer."""
        if index < 0 or index >= self._size:
            raise IndexError(f"Buffer index {index} out of range [0, {self._size})")
        if self._element_type == float:
            offset = index * 8
            return struct.unpack_from('d', self._data, offset)[0]
        else:
            offset = index * 8
            return struct.unpack_from('q', self._data, offset)[0]

    def __setitem__(self, index: int, value):
        """Write element at index into the buffer."""
        if index < 0 or index >= self._size:
            raise IndexError(f"Buffer index {index} out of range [0, {self._size})")
        # Convert immutable bytes to bytearray if needed
        if isinstance(self._data, bytes):
            self._data = bytearray(self._data)
        if self._element_type == float:
            offset = index * 8
            struct.pack_into('d', self._data, offset, float(value))
        else:
            offset = index * 8
            struct.pack_into('q', self._data, offset, int(value))


def to_device(data) -> Buffer:
    """Copy host data to a GPU buffer.

    Accepts: list[int], list[float], array.array, bytes, or any sequence.
    """
    if isinstance(data, bytes):
        return Buffer(data, int, len(data) // 8)
    elif isinstance(data, array.array):
        raw = data.tobytes()
        return Buffer(raw, float if data.typecode in ('f', 'd') else int, len(data))
    elif isinstance(data, (list, tuple)):
        if not data:
            return Buffer(b'', float, 0)
        if isinstance(data[0], float):
            raw = struct.pack(f'{len(data)}d', *data)
            return Buffer(raw, float, len(data))
        else:
            raw = struct.pack(f'{len(data)}q', *data)
            return Buffer(raw, int, len(data))
    else:
        raise TypeError(f"Cannot convert {type(data)} to GPU buffer")


def from_device(buf: Buffer) -> list:
    """Copy GPU buffer back to host as a Python list."""
    if buf.element_type == float:
        count = buf.nbytes // 8
        return list(struct.unpack(f'{count}d', buf._data[:count * 8]))
    else:
        count = buf.nbytes // 8
        return list(struct.unpack(f'{count}q', buf._data[:count * 8]))


def alloc(size: int, dtype: type = float) -> Buffer:
    """Allocate an empty GPU buffer."""
    elem_size = 8  # f64 or i64
    return Buffer(bytes(size * elem_size), dtype, size)


def thread_id() -> int:
    """Get the current GPU thread ID.

    This is a compile-time intrinsic — when compiled by Molt, it maps to:
    - Metal: [[thread_position_in_grid]]
    - WGSL: @builtin(global_invocation_id).x
    - CUDA: blockIdx.x * blockDim.x + threadIdx.x
    - HIP: hipBlockIdx_x * hipBlockDim_x + hipThreadIdx_x

    At Python runtime (interpreted mode), returns 0 for testing.
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

        In interpreted mode: runs the kernel function sequentially for each thread ID.
        When compiled by Molt: dispatches to GPU via the gpu_pipeline.
        """
        grid = self._grid or 256
        threads = self._threads or 256
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

    When compiled by Molt, this generates Metal/WGSL/CUDA/HIP shader code
    and dispatches on the GPU. In interpreted mode, it runs sequentially.
    """
    return _KernelLauncher(func)
