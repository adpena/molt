"""Minimal test.support.os_helper helpers for Molt (partial)."""

from __future__ import annotations

from collections.abc import Iterator
import contextlib
import os
import shutil
import tempfile


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): expand os_helper coverage for file, path, and process helpers used by CPython tests.


TESTFN = "testfile"
TESTFN_NONASCII = "testfile_nonascii"


def unlink(path: str) -> None:
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def rmtree(path: str) -> None:
    shutil.rmtree(path, ignore_errors=True)


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
    "TESTFN_NONASCII",
    "rmtree",
    "temp_dir",
    "unlink",
]
