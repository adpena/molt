"""Live export ownership across array, bytearray, derived views and BytesIO."""

import array
import io
import sys


arr = array.array("h", [1, 2, 3])
view = memoryview(arr)
tail = view[1:]
stride = view[::2]
nested = view[1:][1:]
empty_stride_tail = view[1::2][1:]

print(view.format, view.itemsize, view.shape, view.strides, view.tolist())
print(tail.format, tail.tolist())
print(stride.format, stride.shape, stride.strides, stride.tolist())
print(
    empty_stride_tail.format,
    empty_stride_tail.shape,
    empty_stride_tail.strides,
    empty_stride_tail.tolist(),
)

tail[:] = memoryview(array.array("h", [9, 8]))
print(arr.tolist())

nested[:] = memoryview(array.array("h", [7]))
print(nested.format, nested.shape, nested.strides, nested.tolist())
print(arr.tolist())

try:
    arr.append(4)
except Exception as exc:
    print(type(exc).__name__, str(exc))

view.release()
try:
    arr.append(4)
except Exception as exc:
    print(type(exc).__name__, str(exc))

stride.release()
nested.release()
empty_stride_tail.release()
tail.release()
arr.append(4)
print(arr.tolist())

cast_arr = array.array("h", [11, 22, 33])
cast_root = memoryview(cast_arr)
cast_bytes = cast_root.cast("B")
cast_tail = cast_bytes[cast_root.itemsize :].cast("h")
print(cast_tail.format, cast_tail.tolist())
cast_root.release()
cast_bytes.release()
try:
    cast_arr.append(44)
except Exception as exc:
    print("cast pin", type(exc).__name__, str(exc))
cast_tail.release()
cast_arr.append(44)
print(cast_arr.tolist())

buf = bytearray(b"abc")
buf_view = memoryview(buf)
try:
    buf.append(120)
except Exception as exc:
    print("bytearray append", type(exc).__name__)

buf[0] = 122
print(buf_view.tolist())

try:
    del buf[0]
except Exception as exc:
    print("bytearray del", type(exc).__name__)

try:
    buf += b"x"
except Exception as exc:
    print("bytearray iadd", type(exc).__name__)

try:
    buf *= 0
except Exception as exc:
    print("bytearray imul zero", type(exc).__name__)

try:
    buf *= 2
except Exception as exc:
    print("bytearray imul grow", type(exc).__name__)

buf_view.release()
buf += b"x"
buf *= 2
buf.append(120)
print(memoryview(buf).tolist())

bio = io.BytesIO(b"abc")
bio_view = bio.getbuffer()
try:
    bio.seek(0)
    bio.write(b"Z")
except Exception as exc:
    print("bytesio write same", type(exc).__name__)

try:
    bio.seek(0)
    bio.write(b"")
except Exception as exc:
    print("bytesio write empty", type(exc).__name__)

try:
    bio.writelines([b""])
except Exception as exc:
    print("bytesio writelines empty", type(exc).__name__)

try:
    bio.seek(3)
    bio.write(b"x")
except Exception as exc:
    print("bytesio write", type(exc).__name__)

try:
    bio.truncate(3)
except Exception as exc:
    print("bytesio truncate same", type(exc).__name__)

try:
    bio.truncate(1)
except Exception as exc:
    print("bytesio truncate small", type(exc).__name__)

try:
    bio.truncate(5)
except Exception as exc:
    print("bytesio truncate grow", type(exc).__name__)

bio_view[0] = 121
print(memoryview(bio.getvalue()).tolist())
bio_view.release()
bio.seek(3)
print(bio.write(b"x"), bio.getvalue())
print(bio.truncate(2), bio.getvalue())

bio_close = io.BytesIO(b"abc")
bio_close_view = bio_close.getbuffer()
try:
    bio_close.close()
except Exception as exc:
    print("bytesio close", type(exc).__name__, bio_close.closed)
bio_close_view.release()
bio_close.close()
print("bytesio closed", bio_close.closed)

bio_exit = io.BytesIO(b"abc")
bio_exit_view = bio_exit.getbuffer()
try:
    bio_exit.__exit__(None, None, None)
except Exception as exc:
    print("bytesio exit", type(exc).__name__, bio_exit.closed)
bio_exit_view.release()
bio_exit.close()
print("bytesio exit closed", bio_exit.closed)

bio_text = io.BytesIO(b"abc")
bio_text_view = bio_text.getbuffer()
text = io.TextIOWrapper(bio_text, encoding="utf-8")
try:
    text.close()
except Exception as exc:
    print("text bytesio close", type(exc).__name__, text.closed, bio_text.closed)
bio_text_view.release()
text.close()
print("text bytesio closed", text.closed, bio_text.closed)

bio_buffered = io.BytesIO(b"abc")
bio_buffered_view = bio_buffered.getbuffer()
writer = io.BufferedWriter(bio_buffered, buffer_size=8)
try:
    writer.write(b"Z")
    writer.flush()
except Exception as exc:
    print("buffered bytesio flush", type(exc).__name__, bio_buffered.getvalue())
bio_buffered_view.release()
writer.flush()
print("buffered bytesio flushed", bio_buffered.getvalue())
writer.close()


def observe_mutation(label, initial, mutate):
    owner = bytearray(initial)
    exported = memoryview(owner)
    try:
        result = mutate(owner)
    except Exception as exc:
        print(
            "export-mutation",
            label,
            initial,
            type(exc).__name__,
            str(exc),
            bytes(owner),
        )
    else:
        print("export-mutation", label, initial, result, bytes(owner))
    exported.release()


class ScalarError:
    def __init__(self, error):
        self.error = error

    def __float__(self):
        raise self.error("scalar callback")

    def __index__(self):
        raise self.error("scalar callback")

    def __bool__(self):
        raise self.error("scalar callback")


for label, make_buffer in (
    ("typed", lambda: memoryview(bytearray(8)).cast("H")),
    ("multidimensional", lambda: memoryview(bytearray(8)).cast("B", (2, 4))),
    ("strided", lambda: memoryview(bytearray(8))[::2]),
    ("readonly", lambda: memoryview(bytes(8))),
    ("empty-strided", lambda: memoryview(bytearray(8))[0:0:2]),
    (
        "empty-multidimensional",
        lambda: memoryview(bytearray(8)).cast("B", (2, 4))[0:0:2],
    ),
    ("array", lambda: array.array("H", [0, 0, 0, 0])),
):
    target = make_buffer()
    try:
        count = io.BytesIO(b"abcdefgh").readinto(target)
        print("readinto-buffer", label, count, bytes(target))
    except Exception as exc:
        print("readinto-buffer", label, type(exc).__name__, str(exc))


for fmt in ("B", "f", "d", "?"):
    for error in (TypeError, OverflowError, LookupError):
        owner = bytearray(8)
        exported = memoryview(owner).cast(fmt)
        try:
            exported[0] = ScalarError(error)
        except Exception as exc:
            print(
                "scalar-error",
                fmt,
                error.__name__,
                type(exc).__name__,
                str(exc),
                bytes(owner),
            )
        exported.release()

for live_export in (False, True):
    owner = bytearray(b"abc")
    exported = memoryview(owner) if live_export else None
    try:
        owner[::2] = b""
    except Exception as exc:
        print("extended-empty", live_export, type(exc).__name__, str(exc), bytes(owner))
    else:
        print("extended-empty", live_export, bytes(owner))
    if exported is not None:
        exported.release()


for initial in (b"", b"abc"):
    for label, mutate in (
        ("clear", lambda b: b.clear()),
        ("extend-empty", lambda b: b.extend(b"")),
        ("extend-self", lambda b: b.extend(b)),
        ("imul-zero", lambda b: b.__imul__(0)),
        ("imul-one", lambda b: b.__imul__(1)),
        ("pop", lambda b: b.pop()),
        ("pop-out-of-range", lambda b: b.pop(99)),
        ("remove-missing", lambda b: b.remove(255)),
        ("del-empty", lambda b: b.__delitem__(slice(0, 0))),
        ("replace-same", lambda b: b.__setitem__(slice(None), b"x" * len(b))),
        ("replace-grow", lambda b: b.__setitem__(slice(None), b"x" * (len(b) + 1))),
        ("insert-invalid-both", lambda b: b.insert(None, 999)),
        ("insert-invalid-index", lambda b: b.insert(None, 1)),
        ("insert-invalid-value", lambda b: b.insert(0, 999)),
    ):
        observe_mutation(label, initial, mutate)
    if sys.version_info >= (3, 14):
        observe_mutation("resize-same", initial, lambda b: b.resize(len(b)))
        observe_mutation("resize-negative", initial, lambda b: b.resize(-1))
        observe_mutation("resize-invalid", initial, lambda b: b.resize(None))


for derive in (
    lambda v: memoryview(v),
    lambda v: v[1:],
    lambda v: v[0:0:2],
    lambda v: v.cast("B", (2, 2)),
    lambda v: v.toreadonly(),
):
    owner = bytearray(b"abcd")
    root = memoryview(owner)
    derived = derive(root)
    root.release()
    root.release()
    try:
        owner.append(120)
    except Exception as exc:
        print("derived-pin", derived.shape, derived.strides, type(exc).__name__)
    print("derived-data", derived.tobytes())
    derived.release()
    derived.release()
    owner.append(120)
    print("derived-release", bytes(owner))


events = []


class Index:
    def __init__(self, label, value, action=None):
        self.label = label
        self.value = value
        self.action = action

    def __index__(self):
        events.append(self.label)
        if self.action is not None:
            self.action()
        return self.value


owner = bytearray(b"abc")
owner.insert(Index("insert-index", 0), Index("insert-value", 120))
print("insert-order", events, bytes(owner))


class Values:
    def __init__(self, owner):
        self.owner = owner

    def __iter__(self):
        events.append("rhs-iter")
        self.owner.clear()
        yield 120


for deletion in (False, True):
    events.clear()
    owner = bytearray(b"abc")
    selection = slice(
        Index("start", 1, lambda: owner.clear()),
        Index("stop", 3),
        Index("step", 1),
    )
    try:
        if deletion:
            del owner[selection]
        else:
            owner[selection] = Values(owner)
    except Exception as exc:
        print("slice-order", deletion, events, type(exc).__name__, bytes(owner))
    else:
        print("slice-order", deletion, events, bytes(owner))


for source_kind in ("bytes", "bytearray", "self", "memoryview"):
    events.clear()
    owner = bytearray(b"abc")
    source = {
        "bytes": b"x",
        "bytearray": bytearray(b"x"),
        "self": owner,
        "memoryview": memoryview(b"x"),
    }[source_kind]
    owner[slice(Index("start", 0), Index("stop", 1), Index("step", 1))] = source
    print("slice-source-order", source_kind, events, bytes(owner))

for index, value in (
    (99, 999),
    (99, None),
    (99, Index("scalar-value", 120)),
    (0, Index("scalar-clear", 120, lambda: owner.clear())),
):
    events.clear()
    owner = bytearray(b"abc")
    try:
        owner[index] = value
    except Exception as exc:
        print("scalar-order", index, events, type(exc).__name__, str(exc), bytes(owner))
    else:
        print("scalar-order", index, events, bytes(owner))


for source_offset, destination_offset, count in ((0, 1, 5), (1, 0, 5), (0, 0, 6)):
    stream = io.BytesIO(b"abcdef")
    root = stream.getbuffer()
    destination = root[destination_offset : destination_offset + count]
    stream.seek(source_offset)
    copied = stream.readinto(destination)
    print(
        "readinto-overlap", source_offset, destination_offset, copied, stream.getvalue()
    )
    destination.release()
    root.release()
    stream.close()


class Boolean:
    def __init__(self, action):
        self.action = action

    def __bool__(self):
        events.append("bool")
        self.action()
        return True


class FloatValue:
    def __init__(self, action):
        self.action = action

    def __float__(self):
        events.append("float")
        self.action()
        return 1.25


for fmt, make_value in (
    ("B", lambda action: Index("view-index", 120, action)),
    ("?", Boolean),
    ("f", FloatValue),
):
    events.clear()
    owner = bytearray(4)
    exported = memoryview(owner).cast(fmt)
    try:
        exported[0] = make_value(exported.release)
    except Exception as exc:
        print(
            "view-scalar-release",
            fmt,
            events,
            type(exc).__name__,
            str(exc),
            bytes(owner),
        )
    else:
        print("view-scalar-release", fmt, events, bytes(owner))
    exported.release()
