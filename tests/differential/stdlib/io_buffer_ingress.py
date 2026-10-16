"""CPython oracle for IO buffer roles and callback-visible stream state."""

import io
import os
import tempfile


events = []


class Exporter:
    def __init__(self, data, acquire=None, release=None):
        self.data = data
        self.acquire = acquire
        self.release = release

    def __buffer__(self, flags):
        events.append(("acquire", flags))
        if self.acquire is not None:
            self.acquire()
        return memoryview(self.data)

    def __release_buffer__(self, view):
        events.append("release")
        if self.release is not None:
            self.release()


def show(label, operation):
    events.clear()
    try:
        result = operation()
    except Exception as error:
        result = type(error).__name__
    print(label, result, events)


def initial():
    stream = io.BytesIO(Exporter(b"initial"))
    result = stream.getvalue()
    stream.close()
    return result


def memory_write():
    stream = io.BytesIO()
    count = stream.write(Exporter(bytearray(b"value")))
    result = count, stream.getvalue()
    stream.close()
    return result


def buffered_write():
    raw = io.BytesIO()
    stream = io.BufferedWriter(raw, buffer_size=4)
    value = Exporter(
        b"Q", lambda: stream.write(b"R"), lambda: stream.write(b"S")
    )
    count = stream.write(value)
    stream.flush()
    result = count, raw.getvalue()
    stream.close()
    return result


def closed_write():
    raw = io.BytesIO()
    stream = io.BufferedWriter(raw)
    stream.close()
    return stream.write(Exporter(b"closed"))


def close_during_acquire():
    raw = io.BytesIO()
    stream = io.BufferedWriter(raw)
    return stream.write(Exporter(b"closed", stream.close))


def memory_readinto():
    stream = io.BytesIO(b"read")
    destination = bytearray(4)
    count = stream.readinto(Exporter(destination))
    stream.close()
    return count, destination


def closed_readinto():
    stream = io.BytesIO()
    stream.close()
    return stream.readinto(Exporter(bytearray(4)))


def fail():
    raise RuntimeError("export refused")


def failing_readinto():
    stream = io.BytesIO(b"read")
    try:
        return stream.readinto(Exporter(bytearray(4), fail))
    finally:
        stream.close()


def failing_write():
    stream = io.BytesIO()
    try:
        return stream.write(Exporter(b"write", fail))
    finally:
        stream.close()


def exported_memory():
    stream = io.BytesIO(b"old")
    retained = stream.getbuffer()
    try:
        return stream.write(Exporter(b"new"))
    finally:
        retained.release()
        stream.close()


for label, operation in (
    ("initial", initial),
    ("memory-write", memory_write),
    ("buffered-write", buffered_write),
    ("closed-write", closed_write),
    ("close-in-acquire", close_during_acquire),
    ("memory-readinto", memory_readinto),
    ("closed-readinto", closed_readinto),
    ("failed-readinto", failing_readinto),
    ("failed-write", failing_write),
    ("existing-export", exported_memory),
):
    show(label, operation)


def show_error(label, operation):
    events.clear()
    try:
        operation()
    except Exception as error:
        print(label, type(error).__name__, str(error), events)
    else:
        raise AssertionError("invalid IO input unexpectedly succeeded")


memory = io.BytesIO()
show_error("memory-none", lambda: memory.readinto(None))
show_error("memory-bad-buffer", lambda: memory.readinto(object()))
memory.close()
with tempfile.TemporaryDirectory() as directory:
    raw = io.FileIO(os.path.join(directory, "buffer-role"), "w+")
    show_error("file-none", lambda: raw.readinto(None))
    show_error("file-bad-buffer", lambda: raw.readinto(object()))
    raw.close()


class Descriptor:
    def __index__(self):
        events.append("descriptor-index")
        return -1


show_error("negative-fd-export-error", lambda: os.write(-1, Exporter(b"data", fail)))
show_error("indexed-fd-export-error", lambda: os.write(Descriptor(), Exporter(b"data", fail)))
show_error("fd-overflow-before-export", lambda: os.write(1 << 100, Exporter(b"data")))
