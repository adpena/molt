# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
# MOLT_META: stderr=exact
"""Purpose: differential coverage for io bufferedreader readinto."""

import bz2
import errno
import io
import sys
import tempfile


# The native abstract default is METH_VARARGS, not an omitted implementation:
# it never examines positional values, including after the stream is closed.
class AbstractRaw(io.RawIOBase):
    pass


for abstract_type in (io.RawIOBase, AbstractRaw):
    abstract_raw = abstract_type()
    for closed in (False, True):
        if closed:
            abstract_raw.close()
        for arguments in ((), (None,), (b"readonly",), (None, 1, 2, 3)):
            try:
                abstract_raw.readinto(*arguments)
            except NotImplementedError as error:
                assert error.args == ()
                assert str(error) == ""
                print("raw-default-pos", abstract_type.__name__, closed, len(arguments), error.args)
            else:
                raise AssertionError("abstract readinto accepted positional arguments")
        # Direct CALL and explicit descriptor calls reject keywords before the
        # abstract body, even with extra positional arguments or a closed stream.
        try:
            abstract_raw.readinto(None, 1, extra=2)
        except TypeError as error:
            print("raw-default-keyword", abstract_type.__name__, closed, str(error))
        else:
            raise AssertionError("abstract readinto accepted keyword arguments")
        try:
            io.RawIOBase.readinto(abstract_raw, extra=2)
        except TypeError as error:
            print("raw-default-descriptor-keyword", abstract_type.__name__, closed, str(error))
        else:
            raise AssertionError("abstract descriptor accepted keyword arguments")

with tempfile.NamedTemporaryFile(mode="wb+", delete=True) as handle:
    handle.write(b"abcd")
    handle.flush()
    handle.seek(0)
    reader = io.BufferedReader(handle)
    buf = bytearray(2)
    n = reader.readinto(buf)
    print(n)
    print(bytes(buf))


# Python raw streams must not expose a private native _handle. The fixed source,
# explicit short-count plan and emitted events are independent of buffering.
class Raw(io.RawIOBase):
    def __init__(self, data=b"abcdefgh", plan=()):
        super().__init__()
        self.data = data
        self.position = 0
        self.plan = list(plan)
        self.events = []
        self.callback = None

    def readable(self):
        self.events.append("readable")
        return True

    def seekable(self):
        self.events.append("seekable")
        return True

    def tell(self):
        self.events.append(("tell", self.position))
        return self.position

    def seek(self, offset, whence=0):
        self.events.append(("seek", offset, whence))
        if whence == 0:
            self.position = offset
        elif whence == 1:
            self.position += offset
        else:
            self.position = len(self.data) + offset
        return self.position

    def readinto(self, view):
        is_view = isinstance(view, memoryview)
        self.events.append(
            (
                "readinto",
                len(view),
                view.readonly if is_view else False,
                view.obj is None if is_view else False,
            )
        )
        if self.callback is not None:
            self.callback()
        action = self.plan.pop(0) if self.plan else len(view)
        if isinstance(action, BaseException):
            raise action
        if action is None:
            return None
        count = min(action, len(view), len(self.data) - self.position)
        view[:count] = self.data[self.position : self.position + count]
        self.position += count
        return count

    def close(self):
        self.events.append("close")
        super().close()


def read_calls(raw):
    return sum(
        isinstance(event, tuple) and event[0] == "readinto" for event in raw.events
    )


def require(actual, expected):
    if actual != expected:
        raise AssertionError((actual, expected))


for method in ("read", "read1", "readinto", "readinto1"):
    raw = Raw(plan=(1, 2, 3))
    reader = io.BufferedReader(raw, 4)
    destination = bytearray(b"------")
    if method.startswith("readinto"):
        count = getattr(reader, method)(destination)
        result = count, bytes(destination)
        require(count, 1 if method == "readinto1" else 6)
    else:
        result = getattr(reader, method)(6)
        require(result, b"a" if method == "read1" else b"abcdef")
    if method in ("read1", "readinto1"):
        require(read_calls(raw), 1)
    print("python-raw-short", method, result, raw.events)
    reader.close()
    require(raw.closed, True)

# RawIOBase's inherited readall must remain an ordinary Python raw protocol.
raw = Raw(plan=(2, 1, 3))
reader = io.BufferedReader(raw, 4)
require(reader.read(), b"abcdefgh")
print("python-raw-readall", raw.position, raw.events)
reader.close()

for method in ("read", "read1", "readinto", "readinto1", "peek"):
    raw = Raw(plan=(None,))
    reader = io.BufferedReader(raw, 4)
    destination = bytearray(b"---")
    argument = destination if method.startswith("readinto") else 3
    result = getattr(reader, method)(argument)
    require(result, b"" if method in ("read1", "peek") else None)
    require(bytes(destination), b"---")
    print("python-raw-blocked", method, result, bytes(destination), raw.events)
    reader.close()

for method in ("read", "readinto"):
    raw = Raw(plan=(2, None))
    reader = io.BufferedReader(raw, 4)
    destination = bytearray(b"-----")
    result = getattr(reader, method)(destination if method == "readinto" else 5)
    require(result, 2 if method == "readinto" else b"ab")
    print("python-raw-partial-blocked", method, result, bytes(destination), raw.events)
    reader.close()

raw = Raw(plan=(InterruptedError(errno.EINTR, "retry"), 2))
reader = io.BufferedReader(raw, 4)
require(reader.read1(2), b"ab")
print("python-raw-interrupted", read_calls(raw), raw.events)
reader.close()


class InvalidRaw(Raw):
    def readinto(self, view):
        is_view = isinstance(view, memoryview)
        self.events.append(
            (
                "readinto",
                len(view),
                view.readonly if is_view else False,
                view.obj is None if is_view else False,
            )
        )
        return self.result


for bad in (-1, 9, "invalid", 1 << 100):
    raw = InvalidRaw()
    raw.result = bad
    reader = io.BufferedReader(raw, 4)
    try:
        reader.read1(3)
    except OSError as error:
        print("python-raw-invalid-count", type(error).__name__, str(error))
    else:
        raise AssertionError("invalid raw count accepted")
    reader.close()

raw = Raw(plan=(OSError(errno.EIO, "raw read failed"),))
reader = io.BufferedReader(raw, 4)
try:
    reader.read(2)
except OSError as error:
    require(error.errno, errno.EIO)
    print(
        "python-raw-error", type(error).__name__, error.errno, raw.position, raw.events
    )
else:
    raise AssertionError("raw exception lost")
reader.close()

raw = Raw(plan=(3,))
reader = io.BufferedReader(raw, 4)
peeked = reader.peek(2)
require(peeked, b"abc")
require(reader.tell(), 0)
calls = read_calls(raw)
require(reader.read1(2), b"ab")
require(reader.tell(), 2)
require(reader.readinto1(bytearray(1)), 1)
require(read_calls(raw), calls)
require(reader.tell(), 3)
require(reader.read(2), b"de")
require(reader.seek(4), 4)
require(reader.read(2), b"ef")
require(reader.seek(0), 0)
require(reader.readline(), b"abcdefgh")
print("python-raw-peek-position", peeked, reader.tell(), raw.events)
reader.close()


# Rebinding an exact native raw's method after construction must invalidate the
# callback-free binding proof; it must not bypass the public override.
raw = io.BytesIO(b"native-data")
reader = io.BufferedReader(raw, 4)
original_readinto = raw.readinto
rebound_events = []


def rebound_readinto(view):
    rebound_events.append((len(view), view.readonly, view.obj is None))
    return original_readinto(view)


raw.readinto = rebound_readinto
require(reader.read1(3), b"nat")
require(len(rebound_events), 1)
print("reader-rebound-raw", rebound_events)
reader.close()


# Allocation and initialization are separate, including direct calls and the
# subclass dictionary. Reinitialization discards old read-ahead without closing
# the former raw. A failed initial tell is optional, not an import-time failure.
class Reader(io.BufferedReader):
    pass


construction = []


class Sized:
    def __index__(self):
        construction.append("index")
        return 4


class InitRaw(Raw):
    def readable(self):
        construction.append("readable")
        return True

    def tell(self):
        construction.append("tell")
        raise OSError("position unavailable")


reader = Reader.__new__(Reader)
reader.marker = 17
try:
    reader.read1(1)
except ValueError as error:
    print("reader-uninitialized", type(error).__name__)
else:
    raise AssertionError("uninitialized reader accepted read")
first, second = InitRaw(b"old-data"), InitRaw(b"new-data")
reader.__init__(first, Sized())
require(reader.read(1), b"o")
reader.__init__(second, 4)
require(reader.read(3), b"new")
require(first.closed, False)
print(
    "reader-reinitialize",
    type(reader) is Reader,
    reader.marker,
    reader.raw is second,
    first.position,
    construction,
)
reader.close()
first.close()


class Unreadable(Raw):
    def readable(self):
        self.events.append("readable")
        return False


raw = Unreadable()
reader = Reader.__new__(Reader)
try:
    reader.__init__(raw, 4)
except io.UnsupportedOperation as error:
    print("reader-unreadable", type(error).__name__, reader.raw, raw.closed, raw.events)
else:
    raise AssertionError("unreadable raw accepted")
raw.close()

for size in (0, -1):
    raw = Raw()
    reader = Reader.__new__(Reader)
    try:
        reader.__init__(raw, size)
    except ValueError as error:
        print(
            "reader-invalid-size",
            size,
            type(error).__name__,
            reader.raw is raw,
            raw.closed,
            raw.events,
        )
    else:
        raise AssertionError("nonpositive buffer accepted")
    # Failed initialization owns raw but must not close the caller's object.
    require(raw.closed, False)
    raw.close()

raw = Raw()
reader = io.BufferedReader(raw, 4)
require(reader.read(1), b"a")
require(reader.detach() is raw, True)
require(raw.closed, False)
try:
    reader.read1(1)
except ValueError as error:
    print("reader-detached", type(error).__name__, raw.position, raw.closed)
else:
    raise AssertionError("detached reader accepted read")
raw.close()


class ClosingRaw(Raw):
    def close(self):
        self.events.append("close-error")
        if self.fail:
            raise OSError("raw-close-error")
        super().close()


class FlushReader(Reader):
    def flush(self):
        raise ValueError("reader-flush-error")


raw = ClosingRaw()
raw.fail = True
reader = FlushReader(raw, 4)
try:
    reader.close()
except OSError as error:
    require(type(error.__context__), ValueError)
    print(
        "reader-close-errors",
        str(error),
        str(error.__context__),
        raw.closed,
        raw.events,
    )
else:
    raise AssertionError("close exceptions lost")
raw.fail = False
raw.close()

for action in ("read", "seek", "detach", "close"):
    raw = Raw()
    reader = io.BufferedReader(raw, 4)
    observed = []

    def reenter(raw=raw, action=action, reader=reader, observed=observed):
        raw.callback = None
        try:
            if action == "read":
                reader.read(1)
            elif action == "seek":
                reader.seek(0)
            elif action == "detach":
                reader.detach()
            else:
                reader.close()
        except RuntimeError as error:
            observed.append(type(error).__name__)

    raw.callback = reenter
    result = reader.read1(2)
    require(result, b"ab")
    require(observed, [] if action == "detach" else ["RuntimeError"])
    print("reader-reentry", action, result, observed, raw.closed)
    if action == "detach":
        raw.close()
    else:
        reader.close()


# The callback span is writable and has no owning Python base. CPython permits
# a raw implementation to retain its borrowed view, but using that span after
# return is outside its lifetime. Release retained views without dereferencing
# them; Molt's stronger expiration rule belongs to runtime memory-safety tests.
class ViewRaw(Raw):
    def readinto(self, view):
        child = view[:]
        self.retained = (view, child)
        print("reader-callback-view", view.readonly, child.readonly, view.obj is None)
        view.release()
        child[:3] = b"xyz"
        print("reader-callback-child", bytes(child[:3]))
        return 3


raw = ViewRaw()
reader = io.BufferedReader(raw, 4)
require(reader.read1(3), b"xyz")
for view in raw.retained:
    view.release()
raw.retained = ()
print("reader-callback-released", True)
reader.close()

# Literal BZ2 bytes from CPython3.12.15 bz2.compress(payload, compresslevel=1).
# This is the actual existing compression adapter, not a test stand-in.
if sys.version_info >= (3, 14):
    from compression._common import _streams as compression_streams
else:
    import _compression as compression_streams

encoded = bytes.fromhex(
    "425a68313141592653593cfebdf1000009d1800010000236a59480200022134626c8100"
    "001c164f3d51809c223076ce68bb9229c28481e7f5ef880"
)
source = io.BytesIO(encoded)
raw = compression_streams.DecompressReader(source, bz2.BZ2Decompressor)
reader = io.BufferedReader(raw, 4)
require(reader.read(3), b"rea")
require(reader.readline(), b"der-bridge\n")
require(reader.tell(), 14)
require(reader.seek(0), 0)
require(reader.read(), b"reader-bridge\nline-two\n")
reader.close()
require(source.closed, False)
print("reader-compression-adapter", raw.closed, source.closed)
source.close()


# CHECK_CLOSED preserves already buffered bytes after the raw itself closes.
# Once that exact readahead is exhausted, closure must still refuse a refill.
for method in ("read", "read1", "readinto", "readinto1", "peek", "readline"):
    raw = io.BytesIO(b"abc\ndef")
    reader = io.BufferedReader(raw, 4)
    require(reader.read(1), b"a")
    raw.close()
    destination = bytearray(b"--")
    if method.startswith("readinto"):
        result = getattr(reader, method)(destination)
        require((result, bytes(destination)), (2, b"bc"))
        remaining = b"\n"
    elif method == "peek":
        result = reader.peek(1)
        require(result, b"bc\n")
        remaining = b"bc\n"
    elif method == "readline":
        result = reader.readline()
        require(result, b"bc\n")
        remaining = b""
    else:
        result = getattr(reader, method)(2)
        require(result, b"bc")
        remaining = b"\n"
    if remaining:
        require(reader.read1(), remaining)
    try:
        reader.read1(1)
    except ValueError:
        pass
    else:
        raise AssertionError("closed raw accepted a refill after readahead")
    print("reader-closed-readahead", method, result, bytes(destination))
    reader.close()


# An optional initial tell failure leaves CPython's existing position cache
# unchanged. Seek must not invent another getter call; explicit tell still
# permits buffered-only reads from its callback.
class TellCallbackRaw(Raw):
    def __init__(self):
        super().__init__()
        self.first_tell = True
        self.on_tell = None

    def tell(self):
        self.events.append(("tell", self.position))
        if self.first_tell:
            self.first_tell = False
            raise OSError("initial position unavailable")
        if self.on_tell is not None:
            callback = self.on_tell
            self.on_tell = None
            callback()
        return self.position


raw = TellCallbackRaw()
reader = io.BufferedReader(raw, 4)
require(reader.peek(1), b"abcd")
observed = []


def consume_during_tell():
    observed.append(reader.read1(1))


raw.on_tell = consume_during_tell
require(reader.seek(2), 2)
require(observed, [])
require(reader.read1(1), b"c")
require(reader.tell(), 4)
require(observed, [b"d"])
require(read_calls(raw), 1)
require(
    any(isinstance(event, tuple) and event[0] == "seek" for event in raw.events), False
)
print("reader-fast-seek-tell", observed, raw.events)
reader.close()


# Raw seek-result conversion runs while the operation is owned. Buffered-only
# reads remain legal then; the earlier raw-read reentry negatives still apply
# when another transport operation would actually be necessary.
class SeekPosition:
    def __init__(self, value, callback):
        self.value = value
        self.callback = callback

    def __index__(self):
        if self.callback is not None:
            self.callback()
        return self.value


class PositionCallbackRaw(Raw):
    def seek(self, offset, whence=0):
        value = super().seek(offset, whence)
        return SeekPosition(value, self.on_position)


for method in ("read", "read1", "readinto", "readinto1"):
    raw = PositionCallbackRaw()
    reader = io.BufferedReader(raw, 4)
    require(reader.peek(1), b"abcd")
    observed = []

    def consume_during_position(method=method, reader=reader, observed=observed):
        if method.startswith("readinto"):
            destination = bytearray(1)
            require(getattr(reader, method)(destination), 1)
            observed.append(bytes(destination))
        else:
            observed.append(getattr(reader, method)(1))

    raw.on_position = consume_during_position
    require(reader.seek(0, 2), 8)
    require(observed, [b"a"])
    require(read_calls(raw), 1)
    print("reader-fast-read-position", method, observed, raw.events)
    raw.on_position = None
    reader.close()


# An unbuffered readall forwards the returned Python object, including its
# bytes subclass and empty-object identity. It must not copy through a new Vec.
class ReadAllBytes(bytes):
    pass


class IdentityReadAllRaw(Raw):
    def readall(self):
        self.events.append("readall")
        self.position = len(self.data)
        return self.data


for payload in (ReadAllBytes(b"identity"), ReadAllBytes(b"")):
    raw = IdentityReadAllRaw(payload)
    reader = io.BufferedReader(raw, 4)
    result = reader.read()
    require(result is payload, True)
    require(type(result) is ReadAllBytes, True)
    require(raw.events.count("readall"), 1)
    require(read_calls(raw), 0)
    print("reader-readall-identity", result, type(result).__name__, raw.events)
    reader.close()
