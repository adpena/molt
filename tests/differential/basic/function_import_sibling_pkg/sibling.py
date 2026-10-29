"""Imported only from function bodies of `runner`; prints when its body runs."""

import contextlib

print("sibling body runs")


def compute(value):
    return value + 40


@contextlib.contextmanager
def scope(tag):
    yield "scope:" + tag
