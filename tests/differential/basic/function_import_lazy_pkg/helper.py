"""Imported only from function bodies of the package; prints when it runs."""

import contextlib

print("helper body runs")


def compute(value):
    return value + 100


@contextlib.contextmanager
def scope():
    yield "helper-scope"
