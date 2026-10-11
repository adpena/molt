"""Purpose: differential coverage for pathlib joinpath + os.fspath."""

from pathlib import Path
import os
import io
import sys


def main():
    base = Path("root")
    joined = base.joinpath("a", Path("b"))
    print("join", joined.as_posix())
    print("fspath", os.fspath(joined))
    print("fspath_type", type(os.fspath(joined)).__name__)

    class PathLikeStr:
        def __fspath__(self):
            return "root/like"

    class PathLikeBytes:
        def __fspath__(self):
            return b"root/bytes"

    print("pathlike", str(Path(PathLikeStr())))
    try:
        Path(PathLikeBytes())
        print("pathlike-bytes-ok")
    except TypeError:
        print("pathlike-bytes-typeerror")


if __name__ == "__main__":
    main()


# PyOS_FSPath uses type-level special lookup and returns its owned result
# unchanged. CPython deliberately replaces lookup failures with TypeError,
# while failures from an actual __fspath__ call retain their original identity.
events = []
failure = RuntimeError("path callback failure")
descriptor_failure = AttributeError("path descriptor failure")


class PathResult:
    def __init__(self, value):
        self.value = value

    def __fspath__(self):
        return self.value


class InstanceOnly:
    def __init__(self):
        self.__fspath__ = lambda: events.append("instance spoof")


class FailingPathDescriptor:
    def __get__(self, instance, owner):
        events.append("descriptor")
        raise descriptor_failure


class DescriptorPath:
    __fspath__ = FailingPathDescriptor()


class FailingPath:
    def __fspath__(self):
        events.append("callback")
        raise failure


class NonCallablePath:
    __fspath__ = None


for value in ("root/text", b"root/\xff-bytes"):
    result = os.fspath(PathResult(value))
    assert result is value
    print("fspath owned result", type(result).__name__, repr(result))

for value, expected in (
    (InstanceOnly(), None),
    (NonCallablePath(), None),
    (DescriptorPath(), None),
    (FailingPath(), failure),
):
    events.clear()
    try:
        os.fspath(value)
    except Exception as exc:
        assert isinstance(exc, TypeError) if expected is None else exc is expected
        if isinstance(value, NonCallablePath):
            expected_message = (
                "expected str, bytes or os.PathLike object, not NonCallablePath"
                if sys.version_info >= (3, 13)
                else "'NoneType' object is not callable"
            )
            assert str(exc) == expected_message
        if isinstance(value, DescriptorPath):
            assert (
                str(exc)
                == "expected str, bytes or os.PathLike object, not DescriptorPath"
            )
        print("fspath failure", type(value).__name__, type(exc).__name__, events)
    else:
        raise AssertionError("invalid path was accepted")

# Exercise an actual path consumer as well as the direct protocol operation.
# Both errors occur before an OS filesystem call can be made.
for operation in (os.stat, os.listdir, open, io.FileIO):
    events.clear()
    try:
        operation(FailingPath())
    except RuntimeError as exc:
        assert exc is failure
        assert events == ["callback"]
        print("path consumer failure", operation.__name__, exc is failure)
    else:
        raise AssertionError("path consumer swallowed callback failure")


# CPython _io.open keeps the resolved name; FileIO keeps the original argument.
# /dev/null or NUL avoids creating files and exercises actual handle publication.
for value in (os.devnull, os.fsencode(os.devnull)):
    supplied = PathResult(value)
    with open(supplied, "rb", buffering=0) as stream:
        assert stream.name is value
        print("open name identity", type(value).__name__, stream.name is value)
    with io.FileIO(supplied, "rb") as stream:
        assert stream.name is supplied
        print("FileIO name identity", type(value).__name__, stream.name is supplied)

    def opener(name, flags):
        events.append(name)
        raise failure

    for operation, expected in ((open, value), (io.FileIO, supplied)):
        events.clear()
        try:
            operation(supplied, "rb", opener=opener)
        except RuntimeError as exc:
            assert exc is failure
            assert len(events) == 1 and events[0] is expected
            print("open callback identity", operation.__name__, type(value).__name__)
        else:
            raise AssertionError("opener exception was swallowed")

    for operation in (open, io.FileIO):
        try:
            operation(supplied, "rb", closefd=False)
        except ValueError as exc:
            assert str(exc) == "Cannot use closefd=False with file name"
            print("open closefd rejection", operation.__name__, type(value).__name__)
        else:
            raise AssertionError("filename accepted with closefd=False")


class InvalidOpenerResult:
    def __del__(self):
        events.append("opener result retired")


def invalid_opener(name, flags):
    return InvalidOpenerResult()


for operation in (open, io.FileIO):
    events.clear()
    try:
        operation(os.devnull, "rb", opener=invalid_opener)
    except TypeError:
        assert events == ["opener result retired"]
        print("invalid opener result retired", operation.__name__)
    else:
        raise AssertionError("non-integer opener result was accepted")
