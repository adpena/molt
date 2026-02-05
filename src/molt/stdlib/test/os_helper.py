"""Minimal test.support.os_helper helpers for Molt (partial)."""

from __future__ import annotations

from collections.abc import Iterator
import contextlib
import os
import shutil
import tempfile
import unittest


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand os_helper coverage for file, path, and process helpers used by CPython tests.


TESTFN_ASCII = "testfile"
TESTFN_UNICODE = TESTFN_ASCII + "-\\u00e0\\u00f2\\u0258\\u0141\\u011f"
TESTFN_NONASCII = TESTFN_ASCII + "_nonascii"
TESTFN = TESTFN_ASCII
SAVEDCWD = os.getcwd()


def unlink(path: str) -> None:
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def rmtree(path: str) -> None:
    shutil.rmtree(path, ignore_errors=True)


def rmdir(path: str) -> None:
    try:
        os.rmdir(path)
    except FileNotFoundError:
        pass


def make_bad_fd() -> int:
    file = open(TESTFN, "wb")
    try:
        return file.fileno()
    finally:
        file.close()
        unlink(TESTFN)


_can_symlink: bool | None = None


def can_symlink() -> bool:
    global _can_symlink
    if _can_symlink is not None:
        return _can_symlink
    src = os.path.abspath(TESTFN)
    symlink_path = src + "_can_symlink"
    try:
        os.symlink(src, symlink_path)
        can = True
    except (OSError, NotImplementedError, AttributeError):
        can = False
    else:
        unlink(symlink_path)
    _can_symlink = can
    return can


def skip_unless_symlink(test):
    ok = can_symlink()
    msg = "Requires functional symlink implementation"
    return test if ok else unittest.skip(msg)(test)


@contextlib.contextmanager
def temp_dir(
    path: str | None = None,
    quiet: bool = False,
    *,
    rmtree_func=rmtree,
) -> Iterator[str]:
    del quiet
    if path is None:
        with tempfile.TemporaryDirectory() as tmp:
            yield tmp
            return
    os.makedirs(path, exist_ok=True)
    try:
        yield path
    finally:
        rmtree_func(path)


@contextlib.contextmanager
def temp_cwd(path: str | None = None) -> Iterator[str]:
    with temp_dir(path) as tmp:
        old = os.getcwd()
        os.chdir(tmp)
        try:
            yield tmp
        finally:
            os.chdir(old)


class EnvironmentVarGuard:
    def __init__(self) -> None:
        self._original: dict[str, str] = {}

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        for key in list(os.environ):
            if key not in self._original:
                del os.environ[key]
        os.environ.update(self._original)
        return False

    def set(self, envvar: str, value: str) -> None:
        if envvar not in self._original:
            self._original[envvar] = os.environ.get(envvar, "")
        os.environ[envvar] = value

    def unset(self, envvar: str) -> None:
        if envvar not in self._original:
            self._original[envvar] = os.environ.get(envvar, "")
        os.environ.pop(envvar, None)


__all__ = [
    "EnvironmentVarGuard",
    "TESTFN",
    "TESTFN_ASCII",
    "TESTFN_NONASCII",
    "TESTFN_UNICODE",
    "SAVEDCWD",
    "can_symlink",
    "make_bad_fd",
    "rmtree",
    "rmdir",
    "skip_unless_symlink",
    "temp_cwd",
    "temp_dir",
    "unlink",
]
