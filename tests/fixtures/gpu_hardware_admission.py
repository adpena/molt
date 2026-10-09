import molt.gpu as gpu


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


def refused(func, text):
    try:
        func()
    except RuntimeError as exc:
        assert text in str(exc), str(exc)
    else:
        raise AssertionError("hardware admission unexpectedly succeeded")


refused(lambda: vector_add[1, 5](a, b, c, 5), "bounds")
original_query = gpu.thread_id
gpu.thread_id = lambda: 73
try:
    refused(lambda: vector_add[1, 4](a, b, c, 4), "canonical native primitive")
finally:
    gpu.thread_id = original_query

# A mutable public attribute is not the immutable code descriptor authority.
vector_add._func.__molt_gpu_descriptor__ = "forged user attribute"
vector_add[1, 4](a, b, c, 4)
assert gpu.from_device(c) == [11.0, 22.0, 33.0, 44.0]


class RebindingBuffer:
    @property
    def _data(self):
        gpu.thread_id = lambda: 91
        return a._data

    _format_char = "d"
    _size = 4


try:
    refused(
        lambda: vector_add[1, 4](RebindingBuffer(), b, c, 4),
        "default attribute access",
    )
finally:
    gpu.thread_id = original_query


class FailedBuffer:
    @property
    def _data(self):
        raise KeyError("new GPU attribute failure")


try:
    raise ValueError("handled outer error")
except ValueError:
    try:
        vector_add[1, 4](FailedBuffer(), b, c, 4)
    except KeyError as exc:
        assert "new GPU attribute failure" in str(exc)
    else:
        raise AssertionError("new raised exception was consumed by handled state")


@gpu.kernel
def compare_store(a, b, out, n):
    tid = gpu.thread_id()
    if tid < n:
        out[tid] = a[tid] < b[tid]


refused(lambda: compare_store[1, 4](a, b, c, 4), "strict integral")


@gpu.kernel
def compare_arithmetic(a, b, out, n):
    tid = gpu.thread_id()
    if tid < n:
        out[tid] = (a[tid] < b[tid]) + (b[tid] < a[tid])


refused(lambda: compare_arithmetic[1, 4](a, b, c, 4), "strict integral")


@gpu.kernel
def divide(a, out, factor, n):
    tid = gpu.thread_id()
    if tid < n:
        out[tid] = a[tid] / factor


refused(lambda: divide[1, 4](a, c, 0.0, 4), "div")
refused(lambda: divide[1, 4](a, c, 2.0, 4), "div")

# Aliases preserve actual intrinsic identity without a spelling shortcut.
logical_id = gpu.thread_id


@gpu.kernel
def geometry(tids, blocks, widths, grids, n):
    tid = logical_id()
    gpu.barrier()
    if tid < n:
        tids[tid] = tid
        blocks[tid] = gpu.block_id()
        widths[tid] = gpu.block_dim()
        grids[tid] = gpu.grid_dim()


tids, blocks, widths, grids = (gpu.alloc(4, int) for _ in range(4))
geometry[2, 2](tids, blocks, widths, grids, 4)
assert gpu.from_device(tids) == [0, 1, 2, 3]
assert gpu.from_device(blocks) == [0, 0, 1, 1]
assert gpu.from_device(widths) == [2, 2, 2, 2]
assert gpu.from_device(grids) == [2, 2, 2, 2]


# Scalar-first/interleaved parameters use the same physical binding plan.
@gpu.kernel
def interleaved(n, left, extra, right, out):
    tid = gpu.thread_id()
    if tid < n:
        out[tid] = left[tid] + right[tid] + extra


interleaved[1, 4](4, a, 3, b, c)
assert gpu.from_device(c) == [14.0, 25.0, 36.0, 47.0]


@gpu.kernel
def defaulted(out, value=7):
    tid = gpu.thread_id()
    out[tid] = value


defaulted[1, 4](c)
assert gpu.from_device(c) == [7.0] * 4


@gpu.kernel
def keyword_only(out, *, value):
    out[0] = value


try:
    keyword_only[1, 1](c, 5)
except TypeError:
    pass
else:
    raise AssertionError("hardware bypassed canonical keyword-only binding")


# Shared mutable storage is one device allocation and one host publication.
@gpu.kernel
def alias_update(x, y):
    x[0] = 1
    y[0] = y[0] + 2


shared = gpu.to_device([10.0])
shared._data = bytearray(shared._data)
other = gpu.Buffer(shared._data, float, 1)
backing = shared._data
alias_update[1, 1](shared, other)
assert gpu.from_device(shared) == [3.0]
assert gpu.from_device(other) == [3.0]
assert shared._data is backing and other._data is backing


@gpu.kernel
def retained_load(x, out):
    old = x[0]
    x[0] = 1
    out[0] = old


loaded = gpu.to_device([10.0])
out = gpu.alloc(1, float)
retained_load[1, 1](loaded, out)
assert gpu.from_device(out) == [10.0]
assert gpu.from_device(loaded) == [1.0]


# No store retains bytes identity. A same-value store still performs COW.
@gpu.kernel
def negative_guard(out):
    tid = gpu.thread_id()
    if tid < (0 - 1):
        out[tid] = 1


@gpu.kernel
def same_value(out):
    tid = gpu.thread_id()
    out[tid] = out[tid]


immutable = gpu.to_device([4.0, 5.0])
original = immutable._data
negative_guard[1, 2](immutable)
assert immutable._data is original
same_value[1, 2](immutable)
assert isinstance(immutable._data, bytearray)
assert immutable._data is not original
assert gpu.from_device(immutable) == [4.0, 5.0]

# Zero stores on an empty output preserve its exact immutable identity.
empty = gpu.to_device([])
empty_backing = empty._data
negative_guard[1, 2](empty)
assert empty._data is empty_backing and len(empty._data) == 0

# Repeated arguments to the same immutable wrapper share its one COW owner.
one_wrapper = gpu.to_device([10.0])
alias_update[1, 1](one_wrapper, one_wrapper)
assert gpu.from_device(one_wrapper) == [3.0]

# Distinct wrappers sharing immutable bytes are separate COW owners.
original = gpu.to_device([10.0])._data
left = gpu.Buffer(original, float, 1)
right = gpu.Buffer(original, float, 1)
alias_update[1, 1](left, right)
assert gpu.from_device(left) == [1.0]
assert gpu.from_device(right) == [12.0]
assert left._data is not right._data


# Equal bytes do not merge distinct mutable and immutable storage owners.
immutable_view = gpu.Buffer(original, float, 1)
mutable_view = gpu.Buffer(bytearray(original), float, 1)
mutable_backing = mutable_view._data
alias_update[1, 1](immutable_view, mutable_view)
assert gpu.from_device(immutable_view) == [1.0]
assert gpu.from_device(mutable_view) == [12.0]
assert mutable_view._data is mutable_backing
assert immutable_view._data is not mutable_view._data

# Current normal-path struct bindings are part of the admitted Python methods.
saved_pack_into = gpu.struct.pack_into
gpu.struct.pack_into = lambda *args: None
try:
    untouched = immutable_view._data
    refused(lambda: same_value[1, 1](immutable_view), "struct_pack_into")
    assert immutable_view._data is untouched
finally:
    gpu.struct.pack_into = saved_pack_into


class Overridden(gpu.Buffer):
    def __getitem__(self, index):
        return 100.0


refused(lambda: same_value[1, 1](Overridden(original, float, 1)), "buffer_get")


@gpu.kernel
def racing(out):
    out[0] = gpu.thread_id()


refused(lambda: racing[1, 2](c), "one logical thread")


@gpu.kernel
def multiply(left, right, out):
    tid = gpu.thread_id()
    out[tid] = left[tid] * right[tid]


small = gpu.to_device([2.0])
negative = gpu.to_device([-3.0])
multiply[1, 1](small, negative, out)
assert gpu.from_device(out) == [-6.0]
refused(lambda: multiply[1, 1](gpu.to_device([0.0]), negative, out), "certificate")
for invalid in [0.5, float("inf"), float("nan"), -0.0, float(2**24)]:
    invalid_buffer = gpu.to_device([invalid])
    untouched = invalid_buffer._data
    refused(lambda: same_value[1, 1](invalid_buffer), "strict")
    assert invalid_buffer._data is untouched


# A later argument getter mutates an earlier selected backing. Final admission
# checks the post-callback extent instead of copying from a stale length.
class Shrink(gpu.Buffer):
    pass


later = Shrink(original, float, 1)


def shrink_selected_backing(self):
    backing.clear()
    del Shrink._data
    return self.__dict__["_data"]


Shrink._data = property(shrink_selected_backing)
refused(lambda: alias_update[1, 1](shared, later), "final declared extent")
assert backing == bytearray()


def replacement(a, b, c, n):
    raise AssertionError("replacement body must not execute via a stale descriptor")


vector_add._func.__code__ = replacement.__code__
refused(lambda: vector_add[1, 4](a, b, c, 4), "code descriptor")


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
    original_scalar_output = scalar_output._data
    refused(
        lambda: scalar_add[1, 1](scalar_output, subtype),
        "exact builtin numeric protocol",
    )
    assert scalar_output._data is original_scalar_output
    assert gpu.from_device(scalar_output) == [0]
refused(
    lambda: scalar_reflected[1, 1](scalar_output, ReflectedInt(2)),
    "exact builtin numeric protocol",
)
for exact in (2, 2.0, True):
    scalar_add[1, 1](scalar_output, exact)
    assert gpu.from_device(scalar_output) == [int(exact) + 1]

# Device capability failure must precede every host output publication. This
# launch is numerically/bounds-safe but its workgroup exceeds device limits.
left_before = a._data
right_before = b._data
output_before = c._data
output_values = gpu.from_device(c)
try:
    interleaved[1, 1 << 20](4, a, 3, b, c)
except RuntimeError as exc:
    assert "workgroup" in str(exc).lower(), str(exc)
else:
    raise AssertionError("hardware accepted an over-limit workgroup")
assert a._data is left_before and b._data is right_before
assert c._data is output_before and gpu.from_device(c) == output_values

print("hardware admission ok")
