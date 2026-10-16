# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
import io
import os
import sys

ROOT = os.path.join("logs", "io_text_encodings")
os.makedirs(ROOT, exist_ok=True)
PATH = os.path.join(ROOT, "seek_cookie_utf8.txt")
PATH_SIG = os.path.join(ROOT, "seek_cookie_utf8_sig.txt")

DATA = "alpha\nbeta\r\ngamma\romega\nEND"

with open(PATH, "wb") as handle:
    handle.write(DATA.encode("utf-8"))

with open(PATH_SIG, "w", encoding="utf-8-sig", newline="") as handle:
    handle.write(DATA)


def exercise(label, path, encoding, newline):
    with open(path, "r", encoding=encoding, newline=newline) as handle:
        first = handle.read(5)
        pos = handle.tell()
        mid = handle.read(4)
        handle.seek(pos)
        mid_again = handle.read(4)
        handle.seek(0)
        line1 = handle.readline()
        pos2 = handle.tell()
        line2 = handle.readline()
        handle.seek(pos2)
        line2_again = handle.readline()
        handle.seek(0)
        chunk = handle.read(12)
        pos3 = handle.tell()
        handle.seek(pos3)
        chunk2 = handle.read(5)
    print("case", label, "newline", newline)
    print("read5", repr(first))
    print("read4", repr(mid), "again4", repr(mid_again))
    print("line1", repr(line1))
    print("line2", repr(line2), "line2b", repr(line2_again))
    print("chunk", repr(chunk), "chunk2", repr(chunk2))


for newline in (None, ""):
    exercise("utf-8", PATH, "utf-8", newline)

for newline in (None, ""):
    exercise("utf-8-sig", PATH_SIG, "utf-8-sig", newline)

# Byte positions are observable numbers when no decoder snapshot is needed.
# In particular, a nonzero UTF-8 tell must not be a private tagged integer.
POSITION_PATH = os.path.join(ROOT, "positions.txt")
with open(POSITION_PATH, "w+", encoding="utf-8", newline="") as handle:
    print("raw-start", handle.tell())
    handle.write("é\né\n")
    print("raw-written", handle.tell())
    with open(POSITION_PATH, "rb") as observer:
        print("tell-flushed", observer.read() == b"\xc3\xa9\n\xc3\xa9\n")
    print("raw-seek", handle.seek(3), repr(handle.read()))
    handle.seek(0)
    print("cur-read", repr(handle.read(1)))
    cookie = handle.seek(0, 1)
    handle.seek(0)
    handle.seek(cookie)
    print("cur-restored", repr(handle.read()))

# Restoring after reset must retain the *observed* byte order, including the
# endian opposite to this host. Numeric opaque-cookie equality is irrelevant.
for encoding, explicit, bom in (
    ("utf-16", "utf-16-le", b"\xff\xfe"),
    ("utf-16", "utf-16-be", b"\xfe\xff"),
    ("utf-32", "utf-32-le", b"\xff\xfe\x00\x00"),
    ("utf-32", "utf-32-be", b"\x00\x00\xfe\xff"),
    ("utf-8-sig", "utf-8", b"\xef\xbb\xbf"),
):
    with open(POSITION_PATH, "wb") as handle:
        handle.write(bom + "Aβ\r\nZ".encode(explicit))
    with open(POSITION_PATH, "r", encoding=encoding, newline=None) as handle:
        first = handle.read(1)
        cookie = handle.tell()
        handle.seek(0)
        returned = handle.seek(cookie)
        tail = handle.read()
        print(
            "decoder-reset",
            explicit,
            first == "A",
            returned == cookie,
            tail == "β\nZ",
            handle.encoding == encoding,
        )

# BOM emission belongs to the encoder, independent of the decoder cookie.
# Both repeated writes and nonempty append begin/end at ordinary byte offsets.
for encoding in ("utf-8", "utf-8-sig", "utf-16", "utf-32"):
    expected = "é\né\n".encode(encoding)
    with open(POSITION_PATH, "w", encoding=encoding, newline="") as handle:
        handle.write("é\n")
        handle.write("é\n")
        print("encoder-position", encoding, handle.tell() == len(expected))
    with open(POSITION_PATH, "rb") as handle:
        print("encoder-bytes", encoding, handle.read() == expected)
    with open(POSITION_PATH, "a", encoding=encoding, newline="") as handle:
        print("append-position", encoding, handle.tell() == len(expected))
        handle.write("x")
        print(
            "append-after", encoding, handle.tell() == len("é\né\nx".encode(encoding))
        )
    with open(POSITION_PATH, "rb") as handle:
        print("append-bytes", encoding, handle.read() == "é\né\nx".encode(encoding))
    raw = io.BytesIO(expected)
    raw.seek(0, 2)
    wrapper = io.TextIOWrapper(raw, encoding=encoding, newline="")
    wrapper.write("x")
    wrapper.flush()
    print(
        "wrapped-position",
        encoding,
        wrapper.tell() == len("é\né\nx".encode(encoding)),
        raw.getvalue() == "é\né\nx".encode(encoding),
    )
    wrapper.reconfigure(encoding=encoding)
    wrapper.write("Y")
    wrapper.flush()
    print(
        "encoder-reconfigure", encoding, raw.getvalue() == "é\né\nxY".encode(encoding)
    )
    wrapper.seek(0)
    wrapper.write("B")
    wrapper.flush()
    print("encoder-rewind", encoding, raw.getvalue().startswith("B".encode(encoding)))
    wrapper.close()
    with open(POSITION_PATH, "r+", encoding=encoding, newline="") as handle:
        handle.read(1)
        handle.seek(0, 2)
        handle.write("Y")
        print(
            "end-reset-position",
            encoding,
            handle.tell() == len("é\né\nxY".encode(encoding)),
        )

with open(POSITION_PATH, "wb") as handle:
    handle.write(b"prefix")
for closefd in (False, True):
    descriptor = os.open(POSITION_PATH, os.O_WRONLY)
    with io.FileIO(descriptor, "a", closefd=closefd) as handle:
        print("fd-append-position", closefd, handle.tell())
    if not closefd:
        print("fd-still-owned", os.fstat(descriptor).st_size)
        os.close(descriptor)


def position_opener(path, flags):
    # The constructor, not a pre-positioned test provider, must seek to EOF.
    return os.open(path, os.O_WRONLY)


with io.FileIO(POSITION_PATH, "a", opener=position_opener) as handle:
    print("opener-position", handle.tell())

# Memory positions do not depend on the filesystem's maximum file offset and
# seeking does not allocate the gap. On a 64-bit target this requires a boxed
# integer; on a 32-bit target the memory stream retains its Py_ssize_t limit.
with io.BytesIO(b"prefix") as handle:
    offset = min((1 << 47) + 3, sys.maxsize)
    print("full-width", handle.seek(offset) == offset, handle.tell() == offset)
    try:
        handle.seek(0, 1 << 40)
    except Exception as error:
        print("whence-overflow", type(error).__name__, handle.tell() == offset)

# A host filesystem may refuse a 128 TiB position even without a write. The
# independent descriptor operation defines the expected result/error and cursor
# for the buffered file operation on the same backing filesystem.
offset = (1 << 47) + 3
descriptor = os.open(POSITION_PATH, os.O_RDONLY)
try:
    assert os.lseek(descriptor, 6, os.SEEK_SET) == 6
    expected_errno = None
    try:
        expected_position = os.lseek(descriptor, offset, os.SEEK_SET)
    except OSError as error:
        expected_errno = error.errno
        assert expected_errno is not None
        expected_position = 6
    assert os.lseek(descriptor, 0, os.SEEK_CUR) == expected_position
    for invalid_whence in (1 << 40, -(1 << 40)):
        try:
            os.lseek(descriptor, 0, invalid_whence)
        except OverflowError:
            print(
                "descriptor-whence-overflow",
                os.lseek(descriptor, 0, os.SEEK_CUR) == expected_position,
            )
        else:
            raise AssertionError("descriptor whence must fit C int")
finally:
    os.close(descriptor)

# lseek uses __index__, in fd/offset/whence order, without invoking __int__.
# Every rejected conversion leaves the descriptor position unchanged.
index_events = []


class SeekIndex:
    def __init__(self, name, value, error=None):
        self.name = name
        self.value = value
        self.error = error

    def __index__(self):
        index_events.append(self.name)
        if self.error is not None:
            raise self.error
        return self.value

    def __int__(self):
        raise AssertionError("lseek must use __index__")


class SeekIntOnly:
    def __int__(self):
        index_events.append("int-only")
        return 0


descriptor = os.open(POSITION_PATH, os.O_RDONLY)
try:
    assert (
        os.lseek(
            SeekIndex("fd", descriptor), SeekIndex("offset", 2), SeekIndex("whence", 0)
        )
        == 2
    )
    assert index_events == ["fd", "offset", "whence"]
    print("lseek-index-order", index_events)
    for failing in range(3):
        index_events.clear()
        sentinel = LookupError("lseek-index-sentinel")
        args = [
            SeekIndex("fd", descriptor),
            SeekIndex("offset", 0),
            SeekIndex("whence", 0),
        ]
        args[failing].error = sentinel
        try:
            os.lseek(*args)
        except LookupError as error:
            assert error is sentinel
        else:
            raise AssertionError("index callback failure was lost")
        assert index_events == ["fd", "offset", "whence"][: failing + 1]
        assert os.lseek(descriptor, 0, os.SEEK_CUR) == 2
        print("lseek-index-failure", failing, index_events)
    for failing in range(3):
        for rejected, expected_error in (
            (SeekIntOnly(), TypeError),
            (1.5, TypeError),
            ("0", TypeError),
            (1 << 100, OverflowError),
            (-(1 << 100), OverflowError),
        ):
            index_events.clear()
            args = [
                SeekIndex("fd", descriptor),
                SeekIndex("offset", 0),
                SeekIndex("whence", 0),
            ]
            args[failing] = rejected
            try:
                os.lseek(*args)
            except expected_error:
                pass
            else:
                raise AssertionError("invalid lseek conversion was accepted")
            assert index_events == ["fd", "offset", "whence"][:failing]
            assert os.lseek(descriptor, 0, os.SEEK_CUR) == 2
        print("lseek-rejected-index", failing)
    for rejected, expected_error in (("bad", TypeError), (1 << 100, OverflowError)):
        index_events.clear()
        try:
            os.lseek(
                SeekIndex("fd", descriptor),
                SeekIndex("offset", rejected),
                SeekIndex("whence", 0),
            )
        except expected_error:
            pass
        else:
            raise AssertionError("invalid __index__ result was accepted")
        assert index_events == ["fd", "offset"]
        assert os.lseek(descriptor, 0, os.SEEK_CUR) == 2
        print("lseek-index-result", expected_error.__name__, index_events)
finally:
    os.close(descriptor)

with open(POSITION_PATH, "r+b") as handle:
    assert handle.seek(6) == 6
    try:
        result = handle.seek(offset)
    except OSError as error:
        assert expected_errno is not None
        assert error.errno == expected_errno
        print("host-large-position", "refused", error.errno, handle.tell() == 6)
    else:
        assert expected_errno is None
        assert result == offset
        print(
            "host-large-position",
            "accepted",
            result == expected_position,
            handle.tell() == offset,
        )
    assert handle.tell() == expected_position
with open(POSITION_PATH, "rb") as handle:
    assert handle.read() == b"prefix"


class FlushWitness(io.TextIOWrapper):
    fail = None
    tell_fail = None
    close_during_flush = False

    def flush(self):
        if self.fail is not None:
            raise self.fail
        if self.close_during_flush:
            self.close_during_flush = False
            self.close()
            return None
        return super().flush()

    def tell(self):
        if self.tell_fail is not None:
            raise self.tell_fail
        return super().tell()


raw = io.BytesIO()
wrapper = FlushWitness(raw, encoding="utf-8")
wrapper.write("é")
sentinel = LookupError("tell flush")
wrapper.fail = sentinel
try:
    wrapper.tell()
except Exception as error:
    print("tell-flush-identity", error is sentinel)
for origin in (0, 2):
    before = raw.tell()
    try:
        wrapper.seek(0, origin)
    except Exception as error:
        print("seek-flush-identity", origin, error is sentinel, raw.tell() == before)
wrapper.fail = None
wrapper.tell_fail = sentinel
before = raw.tell()
try:
    wrapper.seek(0, 1)
except Exception as error:
    print("seek-tell-identity", error is sentinel, raw.tell() == before)
wrapper.tell_fail = None
print("tell-flush-recovery", wrapper.tell(), raw.getvalue())
wrapper.close_during_flush = True
try:
    wrapper.tell()
except Exception as error:
    print("tell-flush-closed", type(error).__name__, wrapper.closed)
wrapper.close()


# Memory streams use target Py_ssize_t limits, not the OS file-offset domain.
# No write/allocation at the large position is required to exercise overflow.
for label, stream in (("bytes", io.BytesIO(b"abc")), ("text", io.StringIO("abc"))):
    print(
        "memory-max",
        label,
        stream.seek(sys.maxsize) == sys.maxsize,
        stream.tell() == sys.maxsize,
    )
    for offset, origin in (
        (sys.maxsize + 1, 0),
        (-sys.maxsize - 2, 0),
        (-1, 0),
        (1, 1),
        (sys.maxsize, 2),
    ):
        try:
            stream.seek(offset, origin)
        except Exception as error:
            print(
                "memory-refusal",
                label,
                origin,
                type(error).__name__,
                stream.tell() == sys.maxsize,
            )
    if label == "bytes":
        print("memory-clamp-current", stream.seek(-sys.maxsize - 1, 1))
        print("memory-clamp-end", stream.seek(-4, 2))
    else:
        for offset, origin in ((-1, 1), (-1, 2), (1, 2)):
            try:
                stream.seek(offset, origin)
            except Exception as error:
                print(
                    "string-relative-refusal",
                    origin,
                    type(error).__name__,
                    stream.tell() == sys.maxsize,
                )
    print("memory-end", label, stream.seek(0, 2), stream.tell())
    stream.close()
