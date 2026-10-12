# MOLT_META: stderr=exact platforms=posix
"""Reject malformed semaphore names before any named-resource operation."""

from _multiprocessing import sem_unlink


class Name(str):
    def __contains__(self, value):
        raise AssertionError("argument validation called a string override")


class PathName:
    def __fspath__(self):
        raise AssertionError("semaphore names are not filesystem paths")

    def __str__(self):
        raise AssertionError("semaphore names do not use string coercion")


def observe(label, call):
    try:
        call()
    except (TypeError, ValueError) as error:
        print(label, type(error).__name__)
    else:
        raise AssertionError("invalid semaphore argument was accepted")


for value in (None, 0, b"/molt", PathName()):
    observe("type", lambda: sem_unlink(value))
for value in ("bad\x00name", Name("bad\x00name")):
    observe("nul", lambda: sem_unlink(value))
observe("missing", lambda: sem_unlink())
observe("extra", lambda: sem_unlink("first", "second"))
observe("keyword", lambda: sem_unlink(name="keyword"))
