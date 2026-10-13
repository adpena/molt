"""Purpose: differential coverage for exception control flow."""
# Ensures exceptions from calls stop subsequent statements in the same block.

import sys


def fail():
    raise ValueError("boom")


out = []
try:
    fail()
    out.append("after")
except ValueError:
    out.append("caught")

print(",".join(out))


# Raised errors and the exception currently being handled are distinct owners.
outer = RuntimeError("handled")
inner = ValueError("raised")


class BrokenIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise inner


class MissingTranslation:
    def __getitem__(self, key):
        raise KeyError(key)


class BadKeys:
    def keys(self):
        return 1


def broken_generator():
    yield 1
    raise inner


try:
    raise outer
except RuntimeError:
    for operation in (
        lambda: list(BrokenIterator()),
        lambda: next(iter(BrokenIterator())),
    ):
        try:
            operation()
        except ValueError as error:
            print("raised-owner", error is inner, error.__context__ is outer)
        print("handled-owner", sys.exception() is outer)
    print("translation-lookup", "abc".translate(MissingTranslation()))
    print("mutable-set-key", set() in {frozenset()})
    for operation in (lambda: bytes(BadKeys()), lambda: dict(BadKeys())):
        try:
            operation()
        except TypeError as error:
            print("replacement-error", type(error).__name__, error.__context__ is outer)
        print("handled-owner", sys.exception() is outer)
    generator = broken_generator()
    print("generator-value", next(generator))
    try:
        next(generator)
    except ValueError as error:
        print("generator-error", error is inner, error.__context__ is outer)
    print("handled-owner", sys.exception() is outer)
