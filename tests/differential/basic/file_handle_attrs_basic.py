"""Purpose: verify file handle closefd/buffer attributes parity."""

import io
import os
import tempfile

tmp = tempfile.NamedTemporaryFile(delete=False)
path = tmp.name
tmp.close()

try:
    raw = open(path, "rb", buffering=0)
    print("raw-closefd", hasattr(raw, "closefd"))
    print("raw-closefd-value", raw.closefd)
    print("raw-buffer", hasattr(raw, "buffer"))
    raw.close()

    buf = open(path, "rb")
    print("buf-closefd", hasattr(buf, "closefd"))
    try:
        _ = buf.closefd
        print("buf-closefd-value")
    except AttributeError:
        print("buf-closefd-attrerr")
    print("buf-buffer", hasattr(buf, "buffer"))
    buf.close()

    text = open(path, "r")
    print("text-buffer", hasattr(text, "buffer"))
    print("text-buffer-not-none", text.buffer is not None)
    print("text-closefd", hasattr(text, "closefd"))
    text.close()
finally:
    os.unlink(path)

# The same receiver has distinct base-marker and concrete-resource state.
with tempfile.TemporaryDirectory() as directory:
    path = os.path.join(directory, "lifecycle")
    with open(path, "wb") as stream:
        stream.write(b"abcdef")
    raw = io.FileIO(path, "r")
    print("declaring-base-open", io.IOBase.closed.__get__(raw), raw.closed)
    print(
        "declaring-base-close",
        io.IOBase.close(raw),
        io.IOBase.closed.__get__(raw),
        raw.closed,
    )
    print("declaring-base-resource", raw.read(1))
    try:
        io.IOBase.flush(raw)
    except ValueError as error:
        print("declaring-base-flush", str(error))
    raw.close()
    print("declaring-concrete-close", raw.closed)

    failure = LookupError("close flush")

    class FailingRaw(io.FileIO):
        def flush(self):
            raise failure

    class FailingBuffered(io.BufferedReader):
        def flush(self):
            raise failure

    class FailingText(io.TextIOWrapper):
        def flush(self):
            raise failure

    for stream in (
        FailingRaw(path, "r"),
        FailingBuffered(io.FileIO(path, "r")),
        FailingText(io.FileIO(path, "r")),
    ):
        descriptor = stream.fileno()
        try:
            stream.close()
        except LookupError as error:
            print(
                "concrete-close-error",
                type(stream).__name__,
                error is failure,
                stream.closed,
            )
        try:
            os.fstat(descriptor)
        except OSError:
            print("concrete-descriptor-retired", True)
        print("concrete-close-repeat", stream.close())

    inner_error = RuntimeError("child close")

    class FailingClose(io.FileIO):
        def close(self):
            super().close()
            raise inner_error

    stream = FailingBuffered(FailingClose(path, "r"))
    try:
        stream.close()
    except RuntimeError as error:
        print(
            "concrete-close-chain",
            error is inner_error,
            error.__context__ is failure,
            stream.closed,
        )
    print("concrete-close-chain-repeat", stream.close())

    # Closing a wrapper must not close a borrowed descriptor, but every
    # wrapper must become unusable even after its raw owner was exposed.
    for factory in (
        lambda fd: open(fd, "rb", closefd=False),
        lambda fd: io.BufferedReader(io.FileIO(fd, "r", closefd=False)),
    ):
        descriptor = os.open(path, os.O_RDONLY)
        stream = factory(descriptor)
        raw = stream.raw
        stream.close()
        print("borrowed-child-close", stream.closed, raw.closed, os.read(descriptor, 1))
        try:
            stream.read(1)
        except ValueError:
            print("borrowed-parent-read-refused", True)
        os.close(descriptor)

    child_events = []
    child_error = LookupError("closed getter")

    class ChildClosed(io.FileIO):
        behavior = "normal"

        @property
        def closed(self):
            child_events.append("closed")
            if self.behavior == "true":
                return True
            if self.behavior == "raise":
                raise child_error
            return super().closed

        def close(self):
            child_events.append("close")
            return super().close()

    for factory in (io.BufferedReader, io.TextIOWrapper):
        raw = ChildClosed(path, "r")
        stream = factory(raw)
        raw.behavior = "true"
        child_events.clear()
        print("child-closed-true", factory.__name__, stream.close(), child_events)
        raw.behavior = "raise"
        child_events.clear()
        try:
            stream.close()
        except LookupError as error:
            print("child-closed-error", error is child_error, child_events)
        raw.behavior = "normal"
        stream.close()

    for base in (io.BytesIO, io.StringIO):

        class MemoryClose(base):
            def flush(self):
                raise AssertionError("memory close must not call flush")

        stream = MemoryClose()
        print(
            "memory-close",
            base.__name__,
            stream.close(),
            stream.closed,
            io.IOBase.closed.__get__(stream),
        )

# The base declaration is inherited; abstract owners must not accidentally
# receive the rest of the concrete file method table.
for base in (io.RawIOBase, io.BufferedIOBase, io.TextIOBase):
    print(
        "base-method-ownership",
        base.__name__,
        "close" in base.__dict__,
        "flush" in base.__dict__,
        hasattr(base, "getvalue"),
    )
