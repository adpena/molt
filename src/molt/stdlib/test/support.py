"""Minimal test.support helpers for Molt (partial)."""

from __future__ import annotations

from typing import Any
import contextlib
import gc
import io
import os
import sys
import time
import unittest


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand test.support coverage for CPython regrtest parity.


class _AlwaysEq:
    def __eq__(self, other: Any) -> bool:
        return True

    def __ne__(self, other: Any) -> bool:
        return False


class _NeverEq:
    def __eq__(self, other: Any) -> bool:
        return False

    def __ne__(self, other: Any) -> bool:
        return True

    def __hash__(self) -> int:
        return 1


ALWAYS_EQ = _AlwaysEq()
NEVER_EQ = _NeverEq()
C_RECURSION_LIMIT = sys.getrecursionlimit()
use_resources: set[str] | None = None
verbose = 0
TEST_SUPPORT_DIR = os.path.dirname(os.path.abspath(__file__))
TEST_HOME_DIR = os.path.dirname(TEST_SUPPORT_DIR)

try:
    import socket

    has_socket_support = True
    del socket
except Exception:
    has_socket_support = False


class _CapturedOutput:
    def __init__(self, stream: str) -> None:
        if stream not in {"stdout", "stderr"}:
            raise ValueError(f"unsupported stream: {stream}")
        self._stream = stream
        self._old = None
        self._buffer = io.StringIO()

    def __enter__(self) -> io.StringIO:
        self._old = getattr(sys, self._stream)
        setattr(sys, self._stream, self._buffer)
        return self._buffer

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if self._old is not None:
            setattr(sys, self._stream, self._old)
        return False


def captured_output(stream: str = "stdout") -> _CapturedOutput:
    return _CapturedOutput(stream)


def captured_stdout() -> _CapturedOutput:
    return _CapturedOutput("stdout")


def captured_stderr() -> _CapturedOutput:
    return _CapturedOutput("stderr")


def gc_collect() -> int:
    return gc.collect()


class ResourceDenied(unittest.SkipTest):
    pass


def is_resource_enabled(resource: str) -> bool:
    return use_resources is None or resource in use_resources


def requires(resource: str, msg: str | None = None) -> None:
    if not is_resource_enabled(resource):
        if msg is None:
            msg = f"Use of the {resource!r} resource not enabled"
        raise ResourceDenied(msg)
    if resource in {"network", "urlfetch"} and not has_socket_support:
        raise ResourceDenied("No socket support")


def cpython_only(obj):
    return unittest.skip("CPython only test")(obj)


def findfile(filename: str, subdir: str | None = None) -> str:
    """Try to find a file on sys.path or in the test directory."""
    if os.path.isabs(filename):
        return filename
    if subdir is not None:
        filename = os.path.join(subdir, filename)
    path = [TEST_HOME_DIR] + sys.path
    for dn in path:
        fn = os.path.join(dn, filename)
        if os.path.exists(fn):
            return fn
    return filename


def run_with_tz(tz: str):
    def decorator(func):
        def inner(*args, **kwds):
            try:
                tzset = time.tzset
            except AttributeError:
                raise unittest.SkipTest("tzset required")
            if "TZ" in os.environ:
                orig_tz = os.environ["TZ"]
            else:
                orig_tz = None
            os.environ["TZ"] = tz
            tzset()

            try:
                return func(*args, **kwds)
            finally:
                if orig_tz is None:
                    del os.environ["TZ"]
                else:
                    os.environ["TZ"] = orig_tz
                time.tzset()

        inner.__name__ = func.__name__
        inner.__doc__ = func.__doc__
        return inner

    return decorator


def check_syntax_error(
    testcase,
    statement: str,
    errtext: str = "",
    *,
    lineno: int | None = None,
    offset: int | None = None,
):
    with testcase.assertRaisesRegex(SyntaxError, errtext) as cm:
        compile(statement, "<test string>", "exec")
    err = cm.exception
    testcase.assertIsNotNone(err.lineno)
    if lineno is not None:
        testcase.assertEqual(err.lineno, lineno)
    testcase.assertIsNotNone(err.offset)
    if offset is not None:
        testcase.assertEqual(err.offset, offset)


def check_free_after_iterating(test: Any, iter_func, cls, args: tuple[Any, ...] = ()):
    class _Check(cls):
        def __del__(self):
            nonlocal done
            done = True
            try:
                next(it)
            except StopIteration:
                pass

    done = False
    it = iter_func(_Check(*args))
    test.assertRaises(StopIteration, next, it)
    gc_collect()
    test.assertTrue(done)


@contextlib.contextmanager
def swap_attr(obj: Any, attr: str, new_value: Any):
    old_value = getattr(obj, attr)
    setattr(obj, attr, new_value)
    try:
        yield old_value
    finally:
        setattr(obj, attr, old_value)


@contextlib.contextmanager
def swap_item(mapping: Any, key: Any, new_value: Any):
    sentinel = object()
    old_value = mapping.get(key, sentinel)
    mapping[key] = new_value
    try:
        yield old_value
    finally:
        if old_value is sentinel:
            mapping.pop(key, None)
        else:
            mapping[key] = old_value


def __getattr__(name: str):
    if name.startswith("__"):
        raise AttributeError(name)
    raise RuntimeError(f"MOLT_COMPAT_ERROR: test.support.{name} is not supported")
