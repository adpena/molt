"""Purpose: differential coverage for iterator protocol basic."""


class BadIter:
    def __iter__(self):
        return 123


try:
    iter(BadIter())
    print("ok")
except Exception as exc:
    print(type(exc).__name__)


class BadNext:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError("boom")


it = iter(BadNext())
print(it is iter(it))
try:
    next(it)
except Exception as exc:
    print(type(exc).__name__)
