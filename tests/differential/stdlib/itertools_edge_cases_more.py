"""Purpose: differential coverage for itertools edge cases."""

import itertools

print("islice", list(itertools.islice(range(10), 2, None, 3)))
print("accumulate", list(itertools.accumulate([1, 2, 3])))

it1, it2 = itertools.tee([1, 2, 3], 2)
print("tee1", list(it1))
print("tee2", list(it2))

pairs = [(k, list(g)) for k, g in itertools.groupby("aaabccc")]
print("groupby", pairs)


# Acquisition failure is not the same as an absent iteration protocol. These
# constructors must preserve the exact callback object, including lazy chain
# acquisition and both data/selectors positions of multi-input consumers.
class AcquisitionFailure(ValueError):
    pass


failure = AcquisitionFailure("iterator acquisition")


class FailedIterable:
    def __iter__(self):
        raise failure


class InvalidIterator:
    def __iter__(self):
        return 42


class NotIterable:
    pass


# Capture the selected interpreter's direct protocol diagnostic and expose it
# to the independent CPython/Molt comparison. Constructors must preserve that
# same diagnostic, including upstream wording changes in later Python versions.
invalid_inputs = []
for value in (NotIterable(), InvalidIterator()):
    try:
        iter(value)
    except TypeError as exc:
        diagnostic = str(exc)
        print("direct iterator diagnostic", diagnostic)
        invalid_inputs.append((value, diagnostic))
    else:
        raise AssertionError("invalid direct iterator was accepted")


acquisitions = (
    ("chain", lambda value: list(itertools.chain(value))),
    ("chain.from_iterable", lambda value: list(itertools.chain.from_iterable(value))),
    ("islice", lambda value: list(itertools.islice(value, 1))),
    ("cycle", lambda value: list(itertools.cycle(value))),
    ("accumulate", lambda value: list(itertools.accumulate(value))),
    ("batched", lambda value: list(itertools.batched(value, 2))),
    ("compress.data", lambda value: list(itertools.compress(value, [True]))),
    ("compress.selectors", lambda value: list(itertools.compress([1], value))),
    ("dropwhile", lambda value: list(itertools.dropwhile(bool, value))),
    ("filterfalse", lambda value: list(itertools.filterfalse(None, value))),
    ("pairwise", lambda value: list(itertools.pairwise(value))),
    ("groupby", lambda value: list(itertools.groupby(value))),
    ("starmap", lambda value: list(itertools.starmap(tuple, value))),
    ("takewhile", lambda value: list(itertools.takewhile(bool, value))),
    ("zip_longest", lambda value: list(itertools.zip_longest([1], value))),
    ("tee", lambda value: itertools.tee(value)),
    ("map", lambda value: list(map(bool, value))),
    ("filter", lambda value: list(filter(None, value))),
    ("zip", lambda value: list(zip(value))),
    ("enumerate", lambda value: list(enumerate(value))),
    ("any", any),
    ("all", all),
    # Eager pool construction takes the existing tuple-from-iter owner rather
    # than the raw acquisition branches; these are unaffected sibling controls.
    ("product", lambda value: list(itertools.product(value))),
    ("permutations", lambda value: list(itertools.permutations(value))),
    ("combinations", lambda value: list(itertools.combinations(value, 1))),
    (
        "combinations_with_replacement",
        lambda value: list(itertools.combinations_with_replacement(value, 1)),
    ),
)
for name, acquire in acquisitions:
    try:
        acquire(FailedIterable())
    except BaseException as exc:
        assert exc is failure, (name, type(exc).__name__, str(exc))
    else:
        raise AssertionError((name, "callback was not propagated"))
    for value, diagnostic in invalid_inputs:
        try:
            acquire(value)
        except TypeError as exc:
            assert str(exc) == diagnostic, (name, str(exc))
        else:
            raise AssertionError((name, "invalid protocol was accepted"))
    print("acquisition identity and diagnostics", name, True)


# Fresh first iterator has no external owner. A later failed acquisition must
# release it immediately, with the callback exception still selected.
released = []


class FreshIterator:
    def __init__(self, label):
        self.label = label

    def __iter__(self):
        return self

    def __next__(self):
        raise AssertionError("acquisition must finish before consumption")

    def __del__(self):
        released.append(self.label)


class FreshIterable:
    def __init__(self, label="released"):
        self.label = label

    def __iter__(self):
        return FreshIterator(self.label)


for name, acquire in (
    ("map", lambda first, second: map(bool, first, second)),
    ("zip", lambda first, second: zip(first, second)),
    ("compress", itertools.compress),
    ("zip_longest", itertools.zip_longest),
):
    released.clear()
    try:
        acquire(FreshIterable(), FailedIterable())
    except BaseException as exc:
        assert exc is failure, (name, type(exc).__name__)
    else:
        raise AssertionError((name, "callback was not propagated"))
    assert released == ["released"], (name, released)
    print("partial acquisition released", name, True)


# CPython releases its partially populated iterator tuple from the tail. Two
# successful acquisitions distinguish complete cleanup from one-item cleanup.
for name, acquire in (
    ("map", lambda *values: map(bool, *values)),
    ("zip", zip),
):
    released.clear()
    try:
        acquire(FreshIterable("first"), FreshIterable("second"), FailedIterable())
    except BaseException as exc:
        assert exc is failure, (name, type(exc).__name__)
    else:
        raise AssertionError((name, "third acquisition failure was swallowed"))
    assert released == ["second", "first"], (name, released)
    print("partial acquisition order", name, True)
