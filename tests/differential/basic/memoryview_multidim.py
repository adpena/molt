"""Purpose: differential coverage for memoryview multidim."""

import array
import sys
import traceback


def show_error(fn):
    try:
        fn()
    except Exception as exc:  # noqa: BLE001 - intentional for parity checks
        print(traceback.format_exception_only(type(exc), exc)[0].strip())


ba = bytearray(range(12))
mv = memoryview(ba)
mv2 = mv.cast("B", shape=[3, 4])
print(mv2.shape)
print(mv2.strides)
print(mv2[1, 2])
print(mv2[-1, -1])
show_error(lambda: mv2.count(6))
show_error(lambda: mv2.index(6))
show_error(lambda: mv2[1])
show_error(lambda: mv2[1, 2, 3])
show_error(lambda: mv2[:, 1])
show_error(lambda: mv2[:, :])

mv0 = memoryview(bytearray(b"a")).cast("B", shape=[])
show_error(lambda: len(mv0))
print(mv0[()])
show_error(lambda: mv0[0])

show_error(lambda: mv.cast(">B"))
show_error(lambda: mv.cast("B", shape=[2, 2]))
show_error(lambda: mv.cast("B", shape=1))
show_error(lambda: mv.cast("B", shape=[1, "a"]))
show_error(lambda: mv.cast("B", shape=[0]))

mvh = memoryview(bytearray([0, 0, 0, 0])).cast("H")
mvh[0] = 500
print(mvh[0])

mvc = memoryview(bytearray(b"abc")).cast("c")
print(mvc[0])
mvc[0] = b"z"
print(mvc[0])


def assign_c(val):
    mvc[0] = val


show_error(lambda: assign_c(b"zz"))
show_error(lambda: assign_c(120))


def set_item(view, key, value):
    view[key] = value


for key in (1, (1,), (), (slice(None), slice(None)), slice(None)):
    show_error(lambda: set_item(mv2, key, 0))
show_error(lambda: mv2[(1,)])
show_error(lambda: mv2[()])
show_error(lambda: iter(mv2))
show_error(lambda: iter(mv0))
show_error(lambda: mv2[("bad",)])
show_error(lambda: set_item(mv2, ("bad",), 0))

for rank in (0, 1, 2):
    view = memoryview(bytearray(b"a")).cast("B", shape=[1] * rank)
    view.release()
    show_error(lambda: iter(view))
    show_error(lambda: view[()])
    show_error(lambda: set_item(view, (), 1))

print("zero-dimensional ellipsis", mv0[...] is mv0)
set_item(mv0, Ellipsis, 99)
print("zero-dimensional assignment", mv0[()])


# First-axis slicing retains all trailing dimensions through either lowering.
for sliced in (mv2[0:2], mv2[1:], mv2[0:2:1], mv2[slice(0, 2)], mv2[::-1], mv2[2:0]):
    print("shaped slice", sliced.shape, sliced.strides, sliced.tolist())
print("nested shaped slice", mv2[::-1][0:2].tolist())
show_error(lambda: mv0[:])
show_error(lambda: mv0[::1])
show_error(lambda: mv0[slice(None)])


def typed_loop(view: memoryview):
    values = []
    for item in view:
        values.append(item)
    return values


def typed_comprehension(view: memoryview):
    return [item for item in view]


for view in (mv0, mv2, mv2[:0], mv, mv[:0]):
    for consume in (typed_loop, typed_comprehension):
        show_error(lambda: consume(view))
print("typed byte iteration", typed_loop(mv), typed_comprehension(mv))
print("typed empty iteration", typed_loop(mv[:0]), typed_comprehension(mv[:0]))

# Release failures must not consume iterator positions; prior exhaustion wins.
for consumed in (0, 1, 3):
    view = memoryview(bytearray(b"abc"))
    iterator = iter(view)
    for _ in range(consumed):
        next(iterator)
    view.release()
    for _ in range(5):
        show_error(lambda: next(iterator))


class ReleasingSliceIndex:
    def __init__(self, view, value, calls, owner=None, error=None):
        self.view = view
        self.value = value
        self.calls = calls
        self.owner = owner
        self.error = error

    def __index__(self):
        self.calls.append(self.value)
        self.view.release()
        if self.owner is not None:
            self.owner.append(100)
        if self.error is not None:
            raise self.error("slice callback")
        return self.value


def slice_after_release(view: memoryview, index, mode):
    if mode == 0:
        return view[index:2]
    if mode == 1:
        return view[0:index]
    if mode == 2:
        return view[::index]
    if mode == 3:
        return view[slice(index, 2)]
    return view[2:index]


def show_slice(action):
    try:
        child = action()
        print("slice result", type(child).__name__, child.shape, child.tolist())
        child.release()
    except Exception as error:
        print("slice error", type(error).__name__, str(error))


for mode in range(5):
    owner = bytearray(b"abc")
    view = memoryview(owner)
    calls = []
    index = ReleasingSliceIndex(view, 1, calls)
    show_slice(lambda: slice_after_release(view, index, mode))
    owner.append(100)
    print("slice callback", mode, calls, owner)

# The derived export exists before callbacks: releasing its parent must not
# admit a resize. Unwinding a failed callback must then relinquish that export.
for mode in range(5):
    owner = bytearray(b"abc")
    view = memoryview(owner)
    calls = []
    index = ReleasingSliceIndex(view, 1, calls, owner=owner)
    show_slice(lambda: slice_after_release(view, index, mode))
    owner.append(100)
    print("resize callback cleanup", mode, calls, owner)

for error in (ValueError, LookupError):
    owner = bytearray(b"abc")
    view = memoryview(owner)
    calls = []
    index = ReleasingSliceIndex(view, 1, calls, error=error)
    show_slice(lambda: view[index:2])
    owner.append(100)
    print("failed callback cleanup", error.__name__, calls, owner)

owner = bytearray(b"abc")
view = memoryview(owner)
calls = []
start = ReleasingSliceIndex(view, 0, calls)
stop = ReleasingSliceIndex(view, 3, calls)
step = ReleasingSliceIndex(view, 1, calls)
show_slice(lambda: view[start:stop:step])
owner.append(100)
print("slice callback order", calls, owner)

# Compare addressable shape and contents, not machine-width wrapped strides.
wide = memoryview(bytearray(16)).cast("q")
for step in (sys.maxsize, -sys.maxsize, 2 ** 28, -(2 ** 28)):
    for view in (wide, mv2, wide[:0], wide[::-1]):
        show_slice(lambda: view[::step])
wide.release()

# Empty slices still own the exporter; parent release cannot permit resizing.
for empty in (False, True):
    owner = bytearray(b"abc")
    parent = memoryview(owner)
    child = parent[2:0] if empty else parent[0:2]
    parent.release()
    show_error(lambda: owner.append(100))
    print("derived owner", child.tolist())
    child.release()
    owner.append(100)
    print("released owner", owner)


# Slice assignment must share slicing's singleton/empty geometry. The final
# element is never followed by an unused overflowing pointer increment.
for step in (sys.maxsize, -sys.maxsize, 2 ** 28, -(2 ** 28)):
    owner = bytearray(4)
    view = memoryview(owner)[::2]
    view[::step] = b"\x07"
    print("wide assignment", step > 0, list(owner))
    view[0:0:step] = b""
    view.release()
    owner.append(99)
    print("wide assignment cleanup", list(owner))

owner = bytearray(b"abcdef")
view = memoryview(owner)
view[1:] = view[:-1]
print("overlap contiguous", owner)
view[::2] = view[4::-2]
print("overlap strided", owner)
view[2:2] = b""
view.release()


class AssignmentIndex:
    def __init__(self, label, value, calls, action=None):
        self.label = label
        self.value = value
        self.calls = calls
        self.action = action

    def __index__(self):
        self.calls.append(self.label)
        if self.action is not None:
            self.action()
        return self.value


def assignment_result(view, key, value):
    try:
        view[key] = value
        return "assigned"
    except Exception as error:
        return type(error).__name__, str(error)


words = array.array("h", range(6))
word_view = memoryview(words)
word_view[::2] = array.array("h", [7, 8, 9])
print("multibyte strided assignment", words.tolist())
word_view[::-1] = word_view
print("multibyte reverse self assignment", words.tolist())
word_view.release()

zero_step = memoryview(bytearray(b"abc"))
print("zero step invalid source", assignment_result(zero_step, slice(None, None, 0), object()))
print("zero step valid source", assignment_result(zero_step, slice(None, None, 0), b"x"))
zero_step.release()


# Acquiring the source precedes even step/start/stop conversion. Each failing
# assignment relinquishes its source lease before the final resize.
for kind in ("resize source", "release source", "invalid source", "released source"):
    src_owner = bytearray(b"x")
    src_view = memoryview(src_owner) if kind in ("release source", "released source") else None
    if kind == "released source":
        src_view.release()
    src = object() if kind == "invalid source" else (src_view if src_view is not None else src_owner)
    dst_owner = bytearray(b"abc")
    dst = memoryview(dst_owner)
    calls = []
    action = src_view.release if kind == "release source" else lambda: src_owner.append(99)
    start = AssignmentIndex("start", 0, calls, action)
    key = slice(start, 1, AssignmentIndex("step", 1, calls))
    print("assignment source order", kind, assignment_result(dst, key, src), calls, dst_owner)
    if src_view is not None:
        src_view.release()
    src_owner.append(100)
    print("assignment source cleanup", kind, src_owner)
    dst.release()


def assignment_callback_failure():
    raise LookupError("assignment callback")


# Destination release is allowed, and does not prevent later callbacks or
# release-plus-resize. Callback failures win; release then wins over structure.
for kind in ("release", "resize destination", "later failure", "same source"):
    owner = bytearray(b"abc")
    dst = memoryview(owner)
    calls = []
    def release_destination():
        dst.release()
        if kind == "resize destination":
            owner.append(100)
    start = AssignmentIndex("start", 0, calls, release_destination)
    stop = AssignmentIndex("stop", 1, calls, assignment_callback_failure if kind == "later failure" else None)
    step = AssignmentIndex("step", 1, calls)
    src = dst if kind == "same source" else b"mismatched"
    print("assignment destination order", kind, assignment_result(dst, slice(start, stop, step), src), calls, owner)
    dst.release()
    owner.append(101)
    print("assignment destination cleanup", kind, owner)


# Tuple reads finish bounds/later index callbacks before their release check;
# scalar reads enter memory_item after conversion and recheck release first.
for tuple_key in (False, True):
    for index in (0, 9):
        owner = bytearray(b"abcd")
        view = memoryview(owner)
        calls = []
        key = ReleasingSliceIndex(view, index, calls)
        show_error(lambda: view[(key,)] if tuple_key else view[key])
        print("read key release", tuple_key, index, calls)
        owner.append(99)

for first in (0, 9):
    view = memoryview(bytearray(b"abcd")).cast("B", shape=[2, 2])
    calls = []
    key = ReleasingSliceIndex(view, first, calls)
    later = AssignmentIndex("later", 0, calls, assignment_callback_failure)
    show_error(lambda: view[key, later])
    print("tuple read callback order", first, calls)
