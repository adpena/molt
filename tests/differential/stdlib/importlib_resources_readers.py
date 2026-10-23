"""Purpose: differential coverage for importlib.resources basic reads."""

from importlib import resources


def main():
    text = resources.read_text("importlib", "__init__.py")
    print("has_importlib", "importlib" in text)
    data = resources.read_binary("importlib", "__init__.py")
    print("binary_len", len(data) > 0)


if __name__ == "__main__":
    main()

# readers.only is also the real MultiplexedPath._follow dependency upstream.
from importlib.resources.readers import only
import gc
import weakref


class OnlyFailure(RuntimeError):
    pass


only_failure = OnlyFailure("only callback failed")
references = []


class Token:
    pass


class Values:
    def __init__(self, count, fail=False):
        self.count = count
        self.fail = fail
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls > self.count:
            if self.fail:
                raise only_failure
            raise StopIteration
        value = Token()
        references.append(weakref.ref(value))
        return value


marker = object()
assert only(Values(0), marker) is marker
single = only(Values(1))
assert single is references[-1]()
del single
gc.collect()
assert references[-1]() is None

multiple = Values(3)
try:
    only(multiple)
except ValueError:
    pass
else:
    raise AssertionError("only accepted multiple items")
assert multiple.calls == 2

try:
    only(Values(1, True))
except OnlyFailure as caught:
    assert caught is only_failure
else:
    raise AssertionError("only swallowed callback failure")
# The selected error owns its only() traceback frame and first item in CPython.
# Release that intentional owner before checking the collector for leaks.
only_failure.__traceback__ = None
gc.collect()
assert len(references) == 4 and all(ref() is None for ref in references)
print(
    "only iterator ownership", len(references), all(ref() is None for ref in references)
)
