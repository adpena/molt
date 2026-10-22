"""Purpose: differential coverage for __next__ errors and StopIteration."""

class BadIter:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration


class ErrorIter:
    def __iter__(self):
        return self

    def __next__(self):
        raise RuntimeError("boom")


if __name__ == "__main__":
    it = iter(BadIter())
    try:
        next(it)
        print("stop", "missed")
    except StopIteration:
        print("stop", "ok")

    it2 = iter(ErrorIter())
    try:
        next(it2)
        print("error", "missed")
    except Exception as exc:
        print("error", type(exc).__name__)


# Implicit protocols ignore instance shadows and custom attribute hooks.
class TypeOnly:
    def __init__(self):
        self.remaining = 3
        self.__next__ = lambda: "shadow"

    def __iter__(self):
        return self

    def __getattribute__(self, name):
        if name == "__next__":
            raise AssertionError("instance lookup")
        return object.__getattribute__(self, name)

    def __next__(self):
        if not self.remaining:
            raise StopIteration("complete")
        self.remaining -= 1
        return self


source = TypeOnly()
print("type-only", [item is source for item in source])

# A selected method survives class mutation, reentrant next and collection.
import gc


class MutatingNext:
    def __iter__(self):
        return self

    def __next__(self):
        type(self).__next__ = lambda self: 41
        gc.collect()
        return next(self) + 1


source = MutatingNext()
print("mutation", next(source), next(source))

# Arbitrary descriptors execute once per call and preserve their own callable.
events = []
marker = object()


class NextDescriptor:
    def __get__(self, instance, owner):
        events.append((instance is source, owner is DescriptorIterator))
        if len(events) == 3:
            raise StopIteration(marker)
        return lambda: marker


class DescriptorIterator:
    __next__ = NextDescriptor()

    def __iter__(self):
        return self


source = DescriptorIterator()
print("descriptor", [value is marker for value in source], events)

# Exceptions from descriptor binding and the callable remain exact objects.
for binding in (True, False):
    error = RuntimeError("same error")

    def fail():
        raise error

    class FailingDescriptor:
        def __get__(self, instance, owner):
            if binding:
                raise error
            return fail

    class FailingIterator:
        __next__ = FailingDescriptor()

        def __iter__(self):
            return self

    for consume in (next, list):
        try:
            consume(FailingIterator())
        except RuntimeError as caught:
            print("error identity", binding, consume is next, caught is error)


class StaticNext:
    __next__ = staticmethod(lambda: marker)


class ClassNext:
    @classmethod
    def __next__(cls):
        return cls


print("binding modes", next(StaticNext()) is marker, next(ClassNext()) is ClassNext)

# Sequence fallback has the same descriptor invocation, with one index argument.
class Indexed:
    def __getitem__(self, index):
        if index == 3:
            raise IndexError("end")
        return index + 10


print("sequence", list(Indexed()))

# StopIteration payload identity survives both direct and shared-cache consumers.
import itertools


class Stops:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration(marker)


try:
    next(Stops())
except StopIteration as stopped:
    print("stop payload", stopped.value is marker)
print("tee completion", [list(part) for part in itertools.tee(Stops())])


class WrongReceiver:
    __next__ = type(iter([])).__next__

    def __iter__(self):
        return self


for consume in (next, list):
    try:
        consume(WrongReceiver())
    except TypeError as error:
        print("native receiver", consume is next, str(error))


class NonCallableNext:
    __next__ = 0.0

    def __iter__(self):
        return self


for consume in (next, list):
    try:
        consume(NonCallableNext())
    except TypeError as error:
        print("noncallable", consume is next, str(error))


# Public next must preserve the actual exception, not rebuild one from .value.
class StopsExactly:
    def __init__(self, error):
        self.error = error

    def __iter__(self):
        return self

    def __next__(self):
        raise self.error


payload = (object(), object())
for error in (StopIteration(), StopIteration(payload), StopIteration(payload, marker)):
    source = StopsExactly(error)
    try:
        next(source)
    except StopIteration as caught:
        assert caught is error
        assert caught.args == error.args
        assert caught.value is error.value
        print("exact stop", len(caught.args), caught is error, caught.value is error.value)
    assert next(source, marker) is marker
    assert list(source) == []

error = RuntimeError("default must not mask errors")
try:
    next(StopsExactly(error), marker)
except RuntimeError as caught:
    assert caught is error
    print("default error", caught is error)

for value in (1, [], None):
    for with_default in (False, True):
        try:
            next(value, marker) if with_default else next(value)
        except TypeError as caught:
            print("not iterator", with_default, str(caught))
        else:
            raise AssertionError("non-iterator accepted")

# Fresh values must die after each public next return is released. The direct
# method is an independent control; weakrefs do not retain the referent.
import weakref

class Token:
    pass

class FreshTokens:
    def __init__(self):
        self.refs = []

    def __iter__(self):
        return self

    def __next__(self):
        value = Token()
        self.refs.append(weakref.ref(value))
        return value

for method in (False, True):
    source = FreshTokens()
    for index in range(120):
        value = source.__next__() if method else next(source)
        del value
    alive = [ref() is not None for ref in source.refs]
    assert not any(alive)
    print("fresh retirement", method, sum(alive))

# Native adapters must not erase the original terminal exception before next()
# sees it. Strict zip and tee deliberately normalize their source exhaustion.
for label, adapt, preserves in (
    ("map", lambda source: map(lambda value: value, source), True),
    ("filter", lambda source: filter(None, source), True),
    ("enumerate", enumerate, True),
    ("zip", lambda source: zip(source), True),
    ("strict zip", lambda source: zip(source, strict=True), False),
    ("tee", lambda source: itertools.tee(source)[0], False),
):
    error = StopIteration(payload, marker)
    try:
        next(adapt(StopsExactly(error)))
    except StopIteration as caught:
        assert (caught is error) is preserves
        assert caught.args == (error.args if preserves else ())
        print("adapter stop", label, caught is error, len(caught.args))

class FiniteTokens(FreshTokens):
    def __init__(self, truth):
        super().__init__()
        self.truth = truth
        self.remaining = 12

    def __next__(self):
        if not self.remaining:
            raise StopIteration
        self.remaining -= 1
        value = TruthToken(self.truth)
        self.refs.append(weakref.ref(value))
        return value

class TruthToken(Token):
    def __init__(self, truth):
        self.truth = truth

    def __bool__(self):
        return self.truth

for label in ("any full", "all full", "any short", "all short", "filter", "zip", "strict zip"):
    truth = label in ("all full", "any short")
    source = FiniteTokens(truth)
    if label.startswith("any"):
        assert any(source) is truth
    elif label.startswith("all"):
        assert all(source) is truth
    else:
        stream = filter(None, source) if label == "filter" else zip(source, strict=label == "strict zip")
        for value in stream:
            del value
        del stream
    live = sum(ref() is not None for ref in source.refs)
    assert live == 0
    print("consumer retirement", label, len(source.refs), live)

# zip owns one result cache, including a partial row on failure. map releases
# its partial arguments forward; tuple destruction releases slots in reverse.
retired = []
class RowToken:
    def __init__(self, label, callback=None):
        self.label = label
        self.callback = callback

    def __del__(self):
        retired.append(self.label)
        if self.callback is not None:
            self.callback()

class RowSource:
    def __init__(self, label, fail=False):
        self.label = label
        self.fail = fail
        self.index = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.fail:
            raise RuntimeError("row failure")
        token = RowToken((self.label, self.index))
        self.index += 1
        return token

for label in ("map", "zip", "strict zip"):
    retired.clear()
    sources = (RowSource(0), RowSource(1), RowSource(2, True))
    stream = map(lambda *args: args, *sources) if label == "map" else zip(*sources, strict=label == "strict zip")
    try:
        next(stream)
    except RuntimeError as caught:
        assert str(caught) == "row failure"
    else:
        raise AssertionError("row error lost")
    assert retired == ([(0, 0), (1, 0)] if label == "map" else [])
    del stream
    assert retired == ([(0, 0), (1, 0)] if label == "map" else [(1, 0), (0, 0)])
    print("partial row lifetime", label, retired)

retired.clear()
a, b = RowSource(0), RowSource(1)
stream = zip(a, b)
held = next(stream)
b.fail = True
try:
    next(stream)
except RuntimeError as caught:
    assert str(caught) == "row failure"
else:
    raise AssertionError("aliased partial row error lost")
assert retired == [(0, 1)]  # fresh partial row; aliased cache stays unchanged
assert tuple(value.label for value in held) == ((0, 0), (1, 0))
del stream
assert retired == [(0, 1)]
del held
assert retired == [(0, 1), (1, 0), (0, 0)]
print("aliased partial row", retired)

# An old slot's finalizer advances the same zip. It must see the new slot
# published, and the live outer result must force a fresh nested tuple.
retired.clear()
nested_rows = []
def nested_advance():
    row = next(reentrant_zip)
    nested_rows.append((row[0].label, row[1]))

class ReentrantRows(RowSource):
    def __next__(self):
        token = RowToken(self.index, nested_advance if self.index == 0 else None)
        self.index += 1
        return token

reentrant_zip = zip(ReentrantRows(0), range(10))
row = next(reentrant_zip)
del row
row = next(reentrant_zip)
assert (row[0].label, row[1]) == (1, 2)
assert nested_rows == [(2, 1)]
del row
del reentrant_zip
print("reentrant row publication", nested_rows)

for first_empty in (False, True):
    calls = []
    class StrictSource:
        def __init__(self, index, empty):
            self.index, self.empty = index, empty
        def __iter__(self):
            return self
        def __next__(self):
            calls.append(self.index)
            if self.empty:
                raise StopIteration("terminal value")
            return self.index
    stream = zip(StrictSource(0, first_empty), StrictSource(1, not first_empty), StrictSource(2, False), strict=True)
    try:
        next(stream)
    except ValueError as caught:
        assert str(caught) == "zip() argument 2 is " + ("longer" if first_empty else "shorter") + " than argument 1"
    else:
        raise AssertionError("strict zip mismatch error lost")
    assert calls == [0, 1]
    print("strict first mismatch", first_empty, calls)

# A cache demoted while atomic must be retracked when a row closes a cycle.
import gc
class ZipCycleSource:
    def __iter__(self):
        return self
    def __next__(self):
        value = Token()
        value.stream = self.stream
        self.stream = None
        self.reference = weakref.ref(value)
        return value

source = ZipCycleSource()
stream = zip(source)
source.stream = stream
gc.collect()
row = next(stream)
assert gc.is_tracked(row)
reference = source.reference
del row
del source
del stream
gc.collect()
assert reference() is None
print("zip cache cycle retired", reference() is None)


# Native iterator class descriptors share receiver admission with ordinary
# wrappers; retaining a descriptor must work after the iterator drops its source.
class IndexedSource:
    def __getitem__(self, index):
        if index == 0:
            return 7
        raise IndexError


empty_native_iterator = object()
native_iterators = [
    (iter(IndexedSource()), 7), (iter(lambda: 1, 1), empty_native_iterator),
    (iter(b"a"), 97), (iter(bytearray(b"a")), 97),
    (iter({1: 2}), 1), (iter({1: 2}.values()), 2), (iter({1: 2}.items()), (1, 2)),
    (reversed({1: 2}), 1), (reversed({1: 2}.values()), 2),
    (reversed({1: 2}.items()), (1, 2)), (iter([7]), 7), (reversed([7]), 7),
    (iter(range(1)), 0), (iter(range(2**100, 2**100 + 1)), 2**100),
    (iter({7}), 7), (iter("a"), "a"), (iter("é"), "é"), (iter((7,)), 7),
    (enumerate([7]), (0, 7)), (reversed((7,)), 7), (zip([7]), (7,)),
    (map(abs, [-7]), 7), (filter(None, [7]), 7),
]
for native_iterator, expected_first in native_iterators:
    native_class = type(native_iterator)
    iter_descriptor = native_class.__iter__
    next_descriptor = native_class.__next__
    for name, descriptor in (("__iter__", iter_descriptor), ("__next__", next_descriptor)):
        assert descriptor is native_class.__dict__[name]
        assert descriptor.__objclass__ is native_class
        assert name in dir(native_class)
        assert type(descriptor).__name__ == "wrapper_descriptor"
        assert descriptor.__text_signature__ == "($self, /)"
        for args in ((), (object(),), (native_iterator, 3)):
            try:
                descriptor(*args)
            except TypeError:
                pass
            else:
                raise AssertionError("native descriptor receiver or arity accepted")
        try:
            descriptor(native_iterator, extra=3)
        except TypeError:
            pass
        else:
            raise AssertionError("native descriptor accepted keyword")
    assert iter_descriptor(native_iterator) is native_iterator
    bound_next = native_iterator.__next__
    assert bound_next.__self__ is native_iterator
    assert type(bound_next).__name__ == "method-wrapper"
    if expected_first is empty_native_iterator:
        try:
            bound_next()
        except StopIteration:
            pass
        else:
            raise AssertionError("sentinel iterator yielded a value")
    else:
        assert bound_next() == expected_first
    for advance in (bound_next, lambda: next_descriptor(native_iterator), bound_next):
        try:
            advance()
        except StopIteration:
            pass
        else:
            raise AssertionError("native descriptor failed to exhaust")
        assert type(native_iterator) is native_class
        assert iter_descriptor(native_iterator) is native_iterator
print("native-iterator-descriptors", len(native_iterators))
