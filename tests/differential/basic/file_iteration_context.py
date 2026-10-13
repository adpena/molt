# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,os.getpid,env.read
"""Purpose: differential coverage for file iteration context."""

import gc
import io
import os
import sys
import tempfile
from pathlib import Path


def show(label: str, value) -> None:
    print(label, value)


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / f"molt_open_iter_{os.getpid()}.txt"
path.write_text("one\ntwo\n")

with open(path, "r") as handle:
    show("iter_self", iter(handle) is handle)
    show("iter_lines", [line for line in handle])

handle = open(path, "r")
with handle as inner:
    show("enter_same", inner is handle)
show("closed_after_with", handle.closed)

path.unlink()

# Abstract lifecycle uses public callbacks; context protocol lookup remains on
# the class, while IOBase.__exit__ itself calls the instance's current close.
for base in (io.IOBase, io.RawIOBase, io.BufferedIOBase, io.TextIOBase):
    stream = base()
    print(
        "base-open",
        base.__name__,
        stream.closed,
        stream.flush(),
        stream.__enter__() is stream,
    )
    print("base-exit", stream.__exit__(), stream.closed, stream.close())
    for operation in (stream.flush, stream.__enter__, stream._checkClosed):
        try:
            operation()
        except ValueError as error:
            print("base-closed", str(error))

stream = io.IOBase()
stream.__IOBase_closed = False
print("base-marker-presence", stream.closed, stream.close())

flush_error = LookupError("flush sentinel")
marker_error = RuntimeError("marker sentinel")
events = []


class FlushFailure(io.IOBase):
    def flush(self):
        events.append("flush")
        raise flush_error


stream = FlushFailure()
try:
    stream.close()
except LookupError as error:
    print("base-flush-error", error is flush_error, stream.closed, events)
print("base-repeat-close", stream.close(), events)


class MarkerFailure(FlushFailure):
    def __setattr__(self, name, value):
        if name == "__IOBase_closed":
            events.append("marker")
            raise marker_error
        object.__setattr__(self, name, value)


stream = MarkerFailure()
try:
    stream.close()
except RuntimeError as error:
    print(
        "base-marker-error",
        error is marker_error,
        error.__context__ is flush_error,
        stream.closed,
        events,
    )
# Finalization suppresses the same error without changing a handled exception.
del stream


same_error = ValueError("same close error")


class SameError(io.IOBase):
    def flush(self):
        raise same_error

    def __setattr__(self, name, value):
        if name == "__IOBase_closed":
            raise same_error
        object.__setattr__(self, name, value)


stream = SameError()
try:
    stream.close()
except ValueError as error:
    print(
        "base-same-error-context", error is same_error, error.__context__ is same_error
    )
stream.__dict__["__IOBase_closed"] = True
same_error.__context__ = None


class PublicClosed(io.IOBase):
    @property
    def closed(self):
        return True


stream = PublicClosed()
try:
    stream.__enter__()
except ValueError as error:
    print("public-closed-enter", str(error))
print("public-vs-marker", stream.closed, io.IOBase.closed.__get__(stream))


class CloseResult(io.IOBase):
    def close(self):
        return "close-result"


stream = CloseResult()
print("exit-return", stream.__exit__(None, None, None), stream.__exit__(1, 2, 3, 4))
stream.close = lambda: "instance-close"
print("exit-instance-close", stream.__exit__())


class NativeContext(io.FileIO):
    def __enter__(self):
        events.append("class-enter")
        return "class-result"

    def __exit__(self, kind, value, traceback):
        events.append(("class-exit", kind is RuntimeError, value is marker_error))
        self.close()
        return True


path.write_text("context")
events.clear()
stream = NativeContext(path, "r")
stream.__enter__ = lambda: events.append("instance-enter")
stream.__exit__ = lambda *args: events.append("instance-exit")
with stream as inner:
    print("native-context-enter", inner)
    raise marker_error
print("native-context-exit", events, stream.closed)
path.unlink()

# Use actual refcount/cycle finalization, including resurrection. No fixture
# calls the runtime's retirement helpers or substitutes a manual __del__ call.
events.clear()
resurrected = []


class Finalized(io.IOBase):
    def close(self):
        events.append(("close", self._finalizing, self.closed))
        super().close()


stream = Finalized()
del stream
gc.collect()
print("base-finalized", events)
events.clear()
stream = Finalized()
stream.cycle = stream
del stream
gc.collect()
print("base-cycle-finalized", events)
events.clear()


class Revived(Finalized):
    def close(self):
        resurrected.append(self)
        super().close()


stream = Revived()
del stream
gc.collect()
print("base-resurrected", len(resurrected), resurrected[0].closed, events)
resurrected.clear()
gc.collect()
print("base-finalized-once", events)
events.clear()


class ExplicitFinalizer(Finalized):
    def __del__(self):
        events.append("custom-del")
        super().__del__()


stream = ExplicitFinalizer()
del stream
gc.collect()
print("base-custom-finalizer", events)
events.clear()
try:
    raise marker_error
except RuntimeError as active:
    stream = Finalized()
    del stream
    gc.collect()
    print("base-handled-exception", sys.exception() is active, events)
