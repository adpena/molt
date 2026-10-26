"""Purpose: differential coverage for itertools.tee iterator independence."""

import itertools

it1, it2 = itertools.tee([1, 2, 3], 2)
print("a", next(it1))
print("b", next(it1))
print("c", next(it2))
print("d", list(it1))
print("e", list(it2))

# Each control has a Python-visible oracle independent of Molt's cache layout.
import copy
import gc
import weakref


def check_shared_lifetime():
    first, surviving = itertools.tee(range(140))
    assert next(first) == 0
    first_ref = weakref.ref(first)
    del first
    gc.collect()
    assert first_ref() is None
    assert list(surviving) == list(range(140))
    print("first-reader-retirement", True)


def check_flat_copy():
    source, peer = itertools.tee(range(180))
    for expected in range(61):
        assert next(source) == expected
    nested, = itertools.tee(source, 1)
    copied = copy.copy(source)
    direct = source.__copy__()
    constructed = type(source)(source)
    assert nested is not source
    for reader in (nested, copied, direct, constructed):
        assert next(reader) == 61
    assert next(source) == 61
    assert next(peer) == 0
    source_ref = weakref.ref(source)
    del source
    gc.collect()
    assert source_ref() is None  # Nested readers do not retain another reader.
    for reader in (nested, copied, direct, constructed):
        assert list(reader) == list(range(62, 180))
    assert list(peer) == list(range(1, 180))
    print("flat-copy-and-peek", True)


class Token:
    def __init__(self, value):
        self.value = value


class TokenSource:
    def __init__(self):
        self.index = 0
        self.refs = []

    def __iter__(self):
        return self

    def __next__(self):
        value = Token(self.index)
        self.index += 1
        self.refs.append(weakref.ref(value))
        return value


def check_bounded_retention():
    source = TokenSource()
    a, b = itertools.tee(source)
    peak = 0
    for index in range(200):
        left = next(a)
        right = next(b)
        assert left is right and left.value == index
        del left, right
        live = sum(ref() is not None for ref in source.refs)
        peak = max(peak, live)
    # CPython's block size is 57. This inequality catches an append-only cache
    # without requiring identical allocator sizes or collection statistics.
    assert peak <= 57
    assert all(ref() is None for ref in source.refs[:171])
    del a, b
    gc.collect()
    assert all(ref() is None for ref in source.refs)
    print("bounded-retention", True)


class Resumable:
    def __init__(self):
        self.calls = 0
        self.iter_calls = 0

    def __iter__(self):
        self.iter_calls += 1
        return self

    def __next__(self):
        self.calls += 1
        if self.calls == 1:
            raise StopIteration("once")
        if self.calls == 3:
            raise ValueError("retry")
        return self.calls


def check_errors_and_resume():
    source = Resumable()
    a, b = itertools.tee(source)
    assert source.iter_calls == 2
    assert next(a, "stop") == "stop"
    assert next(b) == 2
    assert next(a) == 2
    try:
        next(b)
    except ValueError as exc:
        assert str(exc) == "retry"
    else:
        raise AssertionError("lost callback error")
    assert next(a) == 4 and next(b) == 4
    assert source.calls == 4
    print("exhaustion-and-callback-retry", True)


class Reentrant:
    def __init__(self):
        self.reader = None
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        try:
            next(self.reader)
        except RuntimeError as exc:
            assert str(exc) == "cannot re-enter the tee iterator"
        else:
            raise AssertionError("recursive tee accepted")
        gc.collect()
        if self.calls == 1:
            raise LookupError("first callback")
        return 42


def check_reentry_and_cycles():
    source = Reentrant()
    first, second = itertools.tee(source)
    source.reader = first
    for reader in (second, first):
        try:
            value = next(reader)
        except LookupError as exc:
            assert str(exc) == "first callback"
        else:
            assert value == 42
    assert next(second) == 42
    first_ref = weakref.ref(first)
    source_ref = weakref.ref(source)
    del first, source
    gc.collect()
    assert first_ref() is not None and source_ref() is not None
    assert next(second) == 42
    del reader, second
    gc.collect()
    assert first_ref() is None and source_ref() is None
    print("reentry-and-mixed-reachability", True)


def check_arguments():
    class Broken:
        def __iter__(self):
            raise LookupError("iter callback")

    class Count:
        def __index__(self):
            return 3

    assert itertools.tee(Broken(), 0) == ()
    for count, error in [(-1, ValueError), (1.5, TypeError)]:
        try:
            itertools.tee(Broken(), count)
        except error:
            pass
        else:
            raise AssertionError("invalid count accepted")
    try:
        itertools.tee(Broken())
    except LookupError as exc:
        assert str(exc) == "iter callback"
    else:
        raise AssertionError("iterator error replaced")
    assert [list(reader) for reader in itertools.tee([5, 6], Count())] == [[5, 6]] * 3
    print("arguments-and-error-identity", True)



def check_constructor_acquisition():
    tee_type = type(itertools.tee(())[0])
    failure = LookupError("acquisition sentinel")

    class Failed:
        def __iter__(self):
            raise failure

    class Missing:
        pass

    class Invalid:
        def __iter__(self):
            return 42

    for construct in (itertools.tee, tee_type):
        try:
            construct(Failed())
        except LookupError as exc:
            assert exc is failure
        else:
            raise AssertionError("acquisition exception lost")
        for source in (Missing(), Invalid()):
            try:
                iter(source)
            except TypeError as exc:
                diagnostic = str(exc)
            else:
                raise AssertionError("invalid direct iterator was accepted")
            print("direct-acquisition-diagnostic", diagnostic)
            try:
                construct(source)
            except TypeError as exc:
                assert str(exc) == diagnostic
            else:
                raise AssertionError("invalid iterator acquisition accepted")

    class SecondIterFails:
        def __init__(self):
            self.calls = 0

        def __iter__(self):
            self.calls += 1
            if self.calls == 2:
                raise failure
            return self

        def __next__(self):
            return 17

    source = SecondIterFails()
    try:
        itertools.tee(source)
    except LookupError as exc:
        assert exc is failure and source.calls == 2
    else:
        raise AssertionError("second acquisition exception lost")
    left, right = itertools.tee(source)
    assert next(left) == next(right) == 17
    print("constructor-acquisition-identity", True)


def check_native_attribute_storage():
    # Nonzero counters and native pointers must never be interpreted as an
    # instance dictionary when resolving an ordinary iterator attribute.
    iterators = (
        itertools.chain([1]), itertools.islice([1], 1), itertools.repeat(1, 2),
        itertools.count(1), itertools.cycle([1]), itertools.accumulate([1]),
        itertools.batched([1], 1), itertools.combinations([1], 1),
        itertools.combinations_with_replacement([1], 1),
        itertools.compress([1], [True]), itertools.dropwhile(bool, [0]),
        itertools.filterfalse(bool, [0]), itertools.pairwise([1, 2]),
        itertools.groupby([1]), itertools.product([1]),
        itertools.permutations([1]), itertools.starmap(pow, [(1, 2)]),
        itertools.takewhile(bool, [1]), itertools.zip_longest([1]),
        itertools.tee([1])[0], next(itertools.groupby([1]))[1],
    )
    for iterator in iterators:
        assert iterator.__iter__() is iterator
        iterator.__next__()
        try:
            iterator.__dict__
        except AttributeError:
            pass
        else:
            raise AssertionError("native iterator exposed a dictionary")
    print("native-iterator-attribute-storage", True)


def check_long_lag_release():
    a, b = itertools.tee(range(20000))
    assert sum(a) == 199990000
    del b  # Retirement of hundreds of links must not recurse with reader lag.
    assert next(a, "done") == "done"
    print("long-lag-release", True)


check_shared_lifetime()
check_flat_copy()
check_bounded_retention()
check_errors_and_resume()
check_reentry_and_cycles()
check_arguments()
check_constructor_acquisition()
check_native_attribute_storage()
check_long_lag_release()


# Value finalizers run at block retirement and may recursively advance the same
# cursor. CPython resets the cursor offset after retiring the previous block.
retirement_reader = None
retirement_events = []


class RetiringToken:
    def __init__(self, value):
        self.value = value

    def __del__(self):
        if self.value == 0 and retirement_reader is not None:
            retirement_events.append(next(retirement_reader).value)


class RetiringSource:
    def __init__(self):
        self.value = 0

    def __iter__(self):
        return self

    def __next__(self):
        value = RetiringToken(self.value)
        self.value += 1
        return value


retiring, = itertools.tee(RetiringSource(), 1)
for _ in range(57):
    next(retiring)
retirement_reader = retiring
assert next(retiring).value == 57
assert retirement_events == [57]
assert next(retiring).value == 58
retirement_reader = None
del retiring
print("retirement-finalizer-reentry", True)


def check_weakref_callback():
    a, b = itertools.tee([11, 12, 13])
    events = []
    ref = weakref.ref(a, lambda ignored: events.append(next(b)))
    del a
    gc.collect()
    assert ref() is None and events == [11]
    assert list(b) == [12, 13]
    print("weakref-callback-peer", True)


check_weakref_callback()
