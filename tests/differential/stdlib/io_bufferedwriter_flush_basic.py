# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for io bufferedwriter flush basic."""

import io
import tempfile

with tempfile.NamedTemporaryFile(mode="wb+", delete=True) as handle:
    writer = io.BufferedWriter(handle)
    writer.write(b"abc")
    writer.flush()
    handle.seek(0)
    print(handle.read())

# A real Python raw protocol, including a wrapper with no private native handle.
class Raw(io.RawIOBase):
    def __init__(self):
        super().__init__()
        self.data = bytearray()
        self.plan = []
        self.events = []
        self.retained = []
        self.callback = None

    def writable(self):
        return True

    def write(self, view):
        self.events.append((view.readonly, view.obj is None, bytes(view)))
        if self.callback is not None:
            self.callback()
        action = self.plan.pop(0) if self.plan else len(view)
        if isinstance(action, BaseException):
            raise action
        if action is None:
            return None
        if isinstance(action, int) and 0 <= action <= len(view):
            self.data.extend(view[:action])
        return action


raw = Raw()
writer = io.BufferedWriter(raw, 4)
print("generic-initial-buffer", writer.write(b"ab"), raw.events)
raw.plan = [2, 2]
print("generic-partial", writer.write(b"cdefgh"), bytes(raw.data))
writer.flush()
print("generic-flushed", bytes(raw.data), raw.events)
print("generic-detach", writer.detach() is raw, raw.closed)
try:
    writer.write(b"!")
except Exception as exc:
    print("generic-detached", type(exc).__name__)
raw.close()

raw = Raw()
writer = io.BufferedWriter(raw, 4)
writer.write(b"abcd")
raw.plan = [None]
try:
    writer.write(b"efg")
except BlockingIOError as exc:
    print("generic-blocked", exc.characters_written, bytes(raw.data))
raw.plan = [2, None]
try:
    writer.flush()
except BlockingIOError as exc:
    print("generic-partial-blocked", exc.characters_written, bytes(raw.data))
raw.plan = [None]
print("generic-blocked-accepted", writer.write(b"ef"))
writer.flush()
print("generic-committed-prefix", bytes(raw.data))
writer.close()

for bad in (-1, 99, "invalid", 1 << 100):
    raw = Raw()
    raw.plan = [bad]
    writer = io.BufferedWriter(raw, 2)
    try:
        writer.write(b"abcd")
    except Exception as exc:
        print("generic-invalid-count", type(exc).__name__, str(exc))
    writer.close()

import errno
raw = Raw()
raw.plan = [InterruptedError(errno.EINTR, "interrupted"), 5]
writer = io.BufferedWriter(raw, 2)
print("generic-interrupt", writer.write(b"abcde"), bytes(raw.data), len(raw.events))
writer.close()

raw = Raw()
writer = io.BufferedWriter(raw, 2)
def reenter():
    try:
        writer.flush()
    except Exception as exc:
        print("generic-reentry", type(exc).__name__)
raw.callback = reenter
writer.write(b"abc")
raw.callback = None
writer.close()

# Native raw subclasses must execute their declared override.
class NativeRaw(io.BytesIO):
    def write(self, data):
        print("native-raw-override", isinstance(data, memoryview), data.readonly, data.obj is None)
        return super().write(data)
raw = NativeRaw()
writer = io.BufferedWriter(raw, 2)
writer.write(b"abcd")
print("native-raw-result", raw.getvalue())
writer.detach()
raw.close()

# __new__ is uninitialized; __init__ owns conversion and replacement, preserving
# the instance dictionary and discarding the previous buffer without flushing.
events = []
class Sized:
    def __index__(self):
        events.append("index")
        return 4
class InitRaw(Raw):
    def writable(self):
        events.append("writable")
        return True
    def tell(self):
        events.append("tell")
        return 0
class Writer(io.BufferedWriter):
    pass
writer = Writer.__new__(Writer)
try:
    writer.write(b"x")
except Exception as exc:
    print("writer-uninitialized", type(exc).__name__)
first, second = InitRaw(), InitRaw()
writer.marker = 17
writer.__init__(first, Sized())
writer.write(b"old")
writer.__init__(second, 4)
writer.write(b"new")
writer.flush()
print("writer-reinitialize", writer.marker, bytes(first.data), bytes(second.data), events)
writer.close()
first.close()

# RawIOBase.write permits the caller to resize/release its input on return.
# Retaining a reference is harmless; its bytes must only be used during write.
events = []
backing = bytearray(b"abcdef")
class ExportingInput:
    def __buffer__(self, flags):
        events.append(("acquire", flags))
        return memoryview(backing)
    def __release_buffer__(self, view):
        events.append(("release", bytes(view)))
class RetainingRaw(Raw):
    def write(self, view):
        child = view[:].cast("B").toreadonly()
        exported = child.__buffer__(0)
        class Proxy:
            def __buffer__(self, flags):
                return child
            def __release_buffer__(self, released):
                events.append("proxy-release")
        proxy_view = memoryview(Proxy())
        self.retained.extend((view, child, exported, memoryview(exported), proxy_view))
        print("callback-view", view.obj is None, child.obj is None, view.readonly)
        view.release()
        backing[1] = ord("Z")
        print("callback-child", bytes(child), hash(child) == hash(b"aZcdef"))
        return len(child)
raw = RetainingRaw()
writer = io.BufferedWriter(raw, 2)
print("callback-count", writer.write(ExportingInput()))
backing.extend(b"!")
print("callback-release", events, backing)
for view in reversed(raw.retained):
    view.release()
writer.close()

# Exact capacity is eager starting in 3.13; record actual raw callback boundaries.
for count in (3, 4, 5):
    raw = Raw()
    writer = io.BufferedWriter(raw, 4)
    print("capacity-boundary", count, writer.write(b"x" * count), len(raw.events))
    writer.flush()
    print("capacity-flushed", count, bytes(raw.data))
    writer.close()

# Reinitialization makes readiness false before the new writable callback.
raw = Raw()
writer = io.BufferedWriter(raw, 4)
class ReinitializingRaw(Raw):
    def writable(self):
        for operation in (lambda: writer.write(b"x"), writer.flush):
            try:
                operation()
            except Exception as exc:
                print("reinit-admission", type(exc).__name__, str(exc))
        return True
replacement = ReinitializingRaw()
writer.__init__(replacement, 4)
writer.close()
raw.close()

class PositionedRaw(Raw):
    def tell(self):
        return len(self.data)
raw = PositionedRaw()
writer = io.BufferedWriter(raw, 4)
writer.write(b"abc")
raw.callback = lambda: print("pending-tell", writer.tell())
raw.plan = [2, 1]
writer.flush()
raw.callback = None
writer.close()

# Exact native classes can acquire an instance write override after creation.
raw = io.BytesIO()
writer = io.BufferedWriter(raw, 2)
def overridden_write(view):
    print("instance-write", view.readonly, view.obj is None, bytes(view))
    return io.BytesIO.write(raw, view)
raw.write = overridden_write
writer.write(b"abc")
print("instance-written", raw.getvalue())
writer.detach()
raw.close()

for source_is_native in (False, True):
    old = io.BytesIO() if source_is_native else Raw()
    replacement = Raw() if source_is_native else io.BytesIO()
    writer = io.BufferedWriter(old, 4)
    class ReplacingInput:
        def __buffer__(self, flags):
            writer.__init__(replacement, 4)
            return memoryview(b"new")
        def __release_buffer__(self, view):
            print("reinit-input-release", bytes(view))
    print("reinit-input", source_is_native, writer.write(ReplacingInput()))
    writer.flush()
    print("reinit-output", bytes(replacement.data) if source_is_native else replacement.getvalue())
    writer.close()
    old.close()

raw = io.BytesIO()
writer = io.BufferedWriter(raw, 4)
writer.write(b"abc")
writer.flush = lambda: print("instance-flush-close")
writer.close()
print("instance-close", raw.closed)

marker = RuntimeError("whence witness")
class WitnessWhence:
    def __index__(self):
        print("uninitialized-whence-index")
        raise marker
for writer_type in (io.BufferedWriter, Writer):
    writer = writer_type.__new__(writer_type)
    try:
        writer.seek(0, WitnessWhence())
    except Exception as exc:
        print("uninitialized-whence-error", exc is marker)

raw = io.BytesIO()
raw.close = lambda: None
inner = io.BufferedWriter(raw, 4)
outer = io.BufferedWriter(inner, 4)
inner.close()
print("nested-public-closed", inner.closed, outer.closed)
print("nested-write", outer.write(b"x"))
try:
    outer.flush()
except Exception as exc:
    print("nested-retired-buffer", type(exc).__name__)
del raw.close
raw.close()
outer.close()

raw = Raw()
inner_raw = Raw()
writer = io.BufferedWriter(raw, 4)
class NestedInitRaw(Raw):
    def writable(self):
        writer.__init__(inner_raw, 8)
        print("nested-initialize", writer.raw is inner_raw)
        return True
outer_raw = NestedInitRaw()
writer.__init__(outer_raw, 4)
print("nested-initialize-result", writer.raw is outer_raw)
writer.write(b"abc")
writer.close()
print("nested-initialize-bytes", bytes(inner_raw.data), bytes(outer_raw.data))
raw.close()
inner_raw.close()

raw = io.BytesIO()
writer = io.BufferedWriter(raw, 4)
print("physical-raw-capabilities", writer.readable(), writer.writable(), writer.seekable())
raw.readable = lambda: "readable witness"
print("physical-raw-query-override", writer.readable())
writer.close()

writer = io.BufferedWriter.__new__(io.BufferedWriter)
print("writer-base-readable-new", writer.readable())
raw = io.BytesIO()
writer.__init__(raw, 4)
writer.detach()
print("writer-base-readable-detached", writer.readable())
raw.close()
raw = io.BytesIO()
writer = io.BufferedWriter(raw, 4)
writer.close()
print("writer-base-readable-closed", writer.readable())

# Observe the actual omitted constructor capacity, not only its public constant.
print("target-default-buffer-size", io.DEFAULT_BUFFER_SIZE)
raw = Raw()
writer = io.BufferedWriter(raw)
writer.write(b"x" * 8193)
print("target-default-write", len(raw.events), len(raw.data))
writer.flush()
print("target-default-flushed", len(raw.data))
writer.close()
