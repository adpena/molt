"""One semantic input executed by CPython, native Molt and split WASM."""

import molt.gpu as gpu
from _intrinsics import runtime_active, require_intrinsic, load_intrinsic
from molt.gpu import thread_id as imported_thread_id


@gpu.kernel
def vector_add(a, b, c, n):
    tid = gpu.thread_id()
    if tid < n:
        c[tid] = a[tid] + b[tid]


a = gpu.to_device([1.0, 2.0, 3.0, 4.0])
b = gpu.to_device([10.0, 20.0, 30.0, 40.0])
c = gpu.alloc(4, float)
vector_add[1, 4](a, b, c, 4)
print(gpu.from_device(c))


class Index:
    def __index__(self):
        return 2


seen = []


@gpu.kernel
def capture():
    seen.append((imported_thread_id(), gpu.block_id(), gpu.block_dim(), gpu.grid_dim()))


capture[{"grid": Index(), "threads": 2}]()
assert seen == [(0, 0, 2, 2), (1, 0, 2, 2), (2, 1, 2, 2), (3, 1, 2, 2)]
seen.clear()
capture[(1,)]()
assert len(seen) == 256 and seen[-1] == (255, 0, 256, 1)
seen.clear()
capture[1]()
assert len(seen) == 256 and seen[-1] == (255, 0, 256, 1)
seen.clear()
for config in [
    True,
    (1, 2, 3),
    {},
    {"blocks": 1},
    {"grid": 1, "threads": False},
    (0, 1),
]:
    try:
        capture[config]()
    except (TypeError, ValueError):
        pass
    else:
        raise AssertionError("invalid config launched")
assert seen == []
try:
    capture[1 << 62, 4]()
except OverflowError:
    pass
else:
    raise AssertionError("overflow launched")
assert seen == []


@gpu.kernel
def inner():
    assert gpu.grid_dim() == 1 and gpu.block_dim() == 1
    raise IndexError("inner failure")


@gpu.kernel
def outer():
    before = (gpu.thread_id(), gpu.block_id(), gpu.block_dim(), gpu.grid_dim())
    try:
        inner[1, 1]()
    except IndexError as exc:
        assert str(exc) == "inner failure"
    else:
        raise AssertionError("inner IndexError swallowed")
    assert (gpu.thread_id(), gpu.block_id(), gpu.block_dim(), gpu.grid_dim()) == before


outer[2, 2]()
assert (gpu.thread_id(), gpu.block_id(), gpu.block_dim(), gpu.grid_dim()) == (
    0,
    0,
    1,
    1,
)


@gpu.kernel
def unguarded(out):
    out[gpu.thread_id()] = 7.0


out = gpu.alloc(1, float)
try:
    unguarded[1, 2](out)
except IndexError:
    pass
else:
    raise AssertionError("out-of-bounds kernel succeeded")
assert gpu.from_device(out) == [7.0]
assert gpu.thread_id() == 0 and gpu.block_dim() == 1


class OtherGPU:
    def thread_id(self):
        return 73


@gpu.kernel
def shadow_parameter(gpu, values):
    values.append(gpu.thread_id())


@gpu.kernel
def shadow_local(values):
    def thread_id():
        return 81

    values.append(thread_id())


values = []
shadow_parameter[1, 1](OtherGPU(), values)
shadow_local[1, 1](values)
query = gpu.thread_id
try:
    gpu.thread_id = lambda: 92

    @gpu.kernel
    def rebound_query(values):
        values.append(gpu.thread_id())

    rebound_query[1, 1](values)
finally:
    gpu.thread_id = query
assert values == [73, 81, 92]


class Replacement:
    def __getitem__(self, config):
        assert config == (3, 5)
        return lambda value: value + 1


capture = Replacement()
assert capture[3, 5](41) == 42


@gpu.kernel
def collective():
    gpu.barrier()


try:
    collective[1, 2]()
except NotImplementedError:
    pass
else:
    raise AssertionError("sequential barrier silently succeeded")
assert gpu.thread_id() == 0 and gpu.block_dim() == 1


# Geometry boxes through the canonical full-range integer owner. Abort at the
# first logical thread so this checks representability without enormous work.
@gpu.kernel
def wide_geometry(expected_grid, expected_threads):
    assert gpu.grid_dim() == expected_grid
    assert gpu.block_dim() == expected_threads
    raise LookupError("stop wide geometry")


for wide_grid, wide_threads in ((1, 1 << 48), (1 << 48, 1)):
    try:
        wide_geometry[wide_grid, wide_threads](wide_grid, wide_threads)
    except LookupError as exc:
        assert str(exc) == "stop wide geometry"
    else:
        raise AssertionError("wide geometry did not stop on its first thread")

# Compiler metadata publication has no guest-callable spelling. This runs in
# both compiled targets; the development CPython lane has no runtime resolver.

if runtime_active():

    def unpublished_code():
        return 7

    try:
        molt_gpu_kernel_descriptor_set(unpublished_code, "forged")
    except NameError:
        pass
    else:
        raise AssertionError("bare metadata ABI name was executable")

    from _intrinsics import molt_gpu_kernel_descriptor_set as imported_setter

    try:
        imported_setter(unpublished_code, "forged")
    except TypeError:
        pass
    else:
        raise AssertionError("imported metadata ABI became a callable")

    try:
        require_intrinsic("molt_gpu_kernel_descriptor_set")
    except RuntimeError:
        pass
    else:
        raise AssertionError("literal lookup exposed metadata publication")
    metadata_name = "molt_gpu_" + "kernel_descriptor_set"
    assert load_intrinsic(metadata_name) is None
    lookup_alias = require_intrinsic
    try:
        lookup_alias(metadata_name)
    except RuntimeError:
        pass
    else:
        raise AssertionError("dynamic lookup exposed metadata publication")


class AddInt(int):
    def __add__(self, other):
        return 99


class AddFloat(float):
    def __add__(self, other):
        return 99.0


class ReflectedInt(int):
    def __radd__(self, other):
        return 77


@gpu.kernel
def scalar_add(out, scalar):
    tid = gpu.thread_id()
    out[tid] = scalar + 1


@gpu.kernel
def scalar_reflected(out, scalar):
    tid = gpu.thread_id()
    out[tid] = 1 + scalar


scalar_output = gpu.alloc(1, int)
for subtype in (AddInt(2), AddFloat(2.0)):
    scalar_add[1, 1](scalar_output, subtype)
    assert gpu.from_device(scalar_output) == [99]
scalar_reflected[1, 1](scalar_output, ReflectedInt(2))
assert gpu.from_device(scalar_output) == [77]

print("launch semantics ok")
