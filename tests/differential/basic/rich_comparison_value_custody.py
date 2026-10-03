"""Rich result identity, reflected dispatch, and explicit truth consumers.

Output is address-free and portable. CPython oracle runs do not attest either
compiled target; replay this same source on native and WASM.
"""

events = []


class Result:
    def __init__(self, name, truth=False, raises=False):
        self.name = name
        self.truth = truth
        self.raises = raises

    def __bool__(self):
        events.append("truth:" + self.name)
        if self.raises:
            raise ValueError("comparison truth")
        return self.truth


class Compared:
    def __init__(self, name, decline=False, raises=False):
        self.name = name
        self.result = Result(name)
        self.decline = decline
        self.raises = raises

    def compare(self, other, symbol):
        events.append(self.name + symbol)
        if self.raises:
            raise ValueError("comparison callback")
        return NotImplemented if self.decline else self.result

    def __eq__(self, other):
        return self.compare(other, "==")

    def __ne__(self, other):
        return self.compare(other, "!=")

    def __lt__(self, other):
        return self.compare(other, "<")

    def __le__(self, other):
        return self.compare(other, "<=")

    def __gt__(self, other):
        return self.compare(other, ">")

    def __ge__(self, other):
        return self.compare(other, ">=")


class Child(Compared):
    pass


def eq(a, b):
    return a == b


def ne(a, b):
    return a != b


def lt(a, b):
    return a < b


def le(a, b):
    return a <= b


def gt(a, b):
    return a > b


def ge(a, b):
    return a >= b


operations = (("eq", eq), ("ne", ne), ("lt", lt), ("le", le), ("gt", gt), ("ge", ge))


def report(label, operation, left, right):
    events.clear()
    try:
        result = operation(left, right)
        if isinstance(result, Result):
            print(label, "result", result.name, events)
        else:
            print(label, "boolean", result, events)
    except (ValueError, TypeError) as error:
        print(label, "error", type(error).__name__, events)


for name, operation in operations:
    left = Compared("left")
    right = Compared("right")
    report(name + ":value", operation, left, right)
    report(name + ":same", operation, left, left)
    report(name + ":none-left", operation, None, right)
    report(name + ":none-right", operation, left, None)
    report(name + ":subtype", operation, left, Child("child"))
    report(name + ":reflected", operation, Compared("decline", decline=True), right)
    report(
        name + ":both-decline",
        operation,
        Compared("a", decline=True),
        Compared("b", decline=True),
    )
    report(name + ":raises", operation, Compared("raises", raises=True), right)

    # Sequence equality consumes truth; ordering returns the selected element's
    # rich result unchanged after testing element equality. The same-element
    # shortcut belongs to container equality, not to a scalar comparison.
    report(name + ":list", operation, [left], [right])
    report(name + ":tuple", operation, (left,), (right,))
    report(name + ":same-list-element", operation, [left], [left])
    report(name + ":same-tuple-element", operation, (left,), (left,))
    left.result.raises = True
    report(name + ":unconsumed-truth", operation, left, right)
    report(name + ":sequence-truth-error", operation, [left], [right])


class DefaultNe:
    def __eq__(self, other):
        events.append("default-eq")
        return Result("default")


report("default-ne", ne, DefaultNe(), DefaultNe())


def discarded_equal(left, right):
    left == right
    left == right
    return True


def repeated_equal(left, right):
    first = left == right
    second = left == right
    return first is second


report("discarded-callbacks", discarded_equal, Compared("left"), Compared("right"))
report("repeated-callbacks", repeated_equal, Compared("left"), Compared("right"))


# Rich comparison values become booleans only at explicit consumers. Errors
# and owned temporaries must survive each consumer's early-exit path.
from operator import countOf


def count_equal(left, right):
    return countOf([left, left], right)


for raises in (False, True):
    left = Compared("consumer")
    right = Compared("other")
    left.result.raises = raises
    report("slice-truth:" + str(raises), eq, slice(left), slice(right))
    report("count-truth:" + str(raises), count_equal, left, right)
    report("count-identity:" + str(raises), count_equal, left, left)


class Container:
    def __init__(self, result):
        self.result = result

    def __contains__(self, item):
        events.append("contains")
        return self.result


def discarded_bool(value):
    bool(value)


def discarded_not(value):
    not value


def discarded_double_not(value):
    not not value


def discarded_in(value):
    0 in Container(value)


def discarded_not_in(value):
    0 not in Container(value)


for name, operation in (
    ("bool", discarded_bool),
    ("not", discarded_not),
    ("double-not", discarded_double_not),
    ("in", discarded_in),
    ("not-in", discarded_not_in),
):
    for raises in (False, True):
        events.clear()
        try:
            operation(Result("discarded", raises=raises))
            outcome = "returned"
        except ValueError:
            outcome = "ValueError"
        print("discarded:" + name, raises, outcome, events)


# Every lexicographic base descriptor has a declaring owner. Subclasses inherit
# storage semantics; explicit base calls bypass outer overrides, while normal
# operators still give a strict right subtype the reflected first attempt.
class TupleChild(tuple):
    pass


class ListChild(list):
    pass


class StringChild(str):
    pass


class BytesChild(bytes):
    pass


class BytearrayChild(bytearray):
    pass


for base, child, a, b in (
    (tuple, TupleChild, (1,), (2,)),
    (list, ListChild, [1], [2]),
    (str, StringChild, "a\ud800", "b\udfff"),
    (bytes, BytesChild, b"a", b"b"),
    (bytearray, BytearrayChild, bytearray(b"a"), bytearray(b"b")),
):
    for name, operation in operations:
        descriptor = getattr(base, "__" + name + "__")
        left = child(a)
        print("inherited", base.__name__, name, operation(left, b), operation(a, child(b)))
        print("declared", base.__name__, name, descriptor(left, b), descriptor(left, 1) is NotImplemented)
        try:
            descriptor(1, b)
        except TypeError:
            print("receiver", base.__name__, name, "TypeError")


class DecliningTuple(tuple):
    def __eq__(self, other):
        events.append("decline-eq")
        return NotImplemented

    def __ne__(self, other):
        events.append("decline-ne")
        return NotImplemented


declining_left = DecliningTuple((1,))
declining_right = DecliningTuple((1,))
for name, operation in (("eq", eq), ("ne", ne)):
    report("tuple-both-decline:" + name, operation, declining_left, declining_right)
    report("tuple-same-decline:" + name, operation, declining_left, declining_left)


class EqualElement:
    def __eq__(self, other):
        events.append("element-eq")
        return True


for base in (tuple, list):
    report("unequal-length:" + base.__name__, eq, base([EqualElement()]), base([EqualElement(), 0]))


class ClearsComparedList:
    def __eq__(self, other):
        events.append("clear-eq")
        mutated_left.clear()
        return False


mutated_left = [ClearsComparedList()]
report("list-reload-after-equality", lt, mutated_left, [object()])

for peer in (b"b", bytearray(b"b"), memoryview(b"b"), memoryview(b"abc")[::2]):
    print("buffer-descriptor", type(peer).__name__, bytes.__lt__(b"a", peer), bytearray.__lt__(bytearray(b"a"), peer))
released_view = memoryview(b"b")
released_view.release()
print("released-buffer-descriptor", bytearray.__eq__(bytearray(b"a"), released_view) is NotImplemented)

from array import array

buffer_array = array("B", [97])
print("array-buffer-descriptor", bytearray.__eq__(bytearray(b"a"), buffer_array))
buffer_array.append(98)
print("array-buffer-released", bytearray.__lt__(bytearray(b"a"), buffer_array))


class MisleadingName(str):
    def __eq__(self, other):
        events.append("name-equality")
        return True

    __hash__ = str.__hash__


def named_function():
    pass


named_function.ordinary = "ordinary-value"
events.clear()
print("attribute-name-content", getattr(named_function, MisleadingName("ordinary")))
print("attribute-name-events", events)


class EqualityMetaclass(type):
    def __eq__(self, other):
        raise ValueError("metaclass equality must not deduplicate a union")

    __hash__ = type.__hash__


class UnionFirst(metaclass=EqualityMetaclass):
    pass


class UnionSecond(metaclass=EqualityMetaclass):
    pass


identity_union = UnionFirst | UnionSecond
print("union-type-identity", len(identity_union.__args__))
print("union-reuses-left", (identity_union | UnionFirst) is identity_union)

import copyreg


class RegistryCode(int):
    def __eq__(self, other):
        events.append("registry-eq")
        raise ValueError("registry equality")

    def __ne__(self, other):
        events.append("registry-ne")
        raise ValueError("registry inequality")


registry_key = ("comparison_fixture", "registered_value")
registry_code = 735192
copyreg._extension_registry[registry_key] = RegistryCode(registry_code)
copyreg._inverted_registry[registry_code] = registry_key
try:
    for registry_operation in (copyreg.add_extension, copyreg.remove_extension):
        events.clear()
        try:
            registry_operation(registry_key[0], registry_key[1], registry_code)
        except ValueError as error:
            print("registry-error", str(error), events)
finally:
    copyreg._extension_registry.pop(registry_key, None)
    copyreg._inverted_registry.pop(registry_code, None)



# Declaring builtin comparison families share one storage contract with the
# exact-type operator path. Inherited dispatch and explicit base calls differ.
from array import array
from types import GenericAlias, UnionType

comparison_slots = ("__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__")
family_receivers = (
    ("int", int, 3),
    ("float", float, 3.0),
    ("complex", complex, 3j),
    ("dict", dict, {1: 2}),
    ("set", set, {1}),
    ("frozenset", frozenset, frozenset({1})),
    ("range", range, range(3)),
    ("slice", slice, slice(1, 3)),
    ("memoryview", memoryview, memoryview(b"a")),
    ("alias", GenericAlias, list[int]),
    ("union", UnionType, int | str),
    ("keys", type({}.keys()), {1: 2}.keys()),
    ("items", type({}.items()), {1: 2}.items()),
)
for family, owner, receiver in family_receivers:
    print("declaring-slots", family, tuple(name in owner.__dict__ for name in comparison_slots))
    try:
        owner.__eq__(object(), receiver)
        wrong_receiver = "accepted"
    except TypeError:
        wrong_receiver = "TypeError"
    print("declaring-admission", family, wrong_receiver, owner.__eq__(receiver, object()) is NotImplemented)


class ComparisonInt(int):
    def __eq__(self, other):
        return "int-override"


class ComparisonFloat(float):
    pass


class ComparisonComplex(complex):
    pass


class ComparisonDict(dict):
    pass


class ComparisonSet(set):
    pass


class ComparisonFrozen(frozenset):
    pass


print("declaring-base", ComparisonInt(7) == 7, int.__eq__(ComparisonInt(7), 7))
print("inherited-families", ComparisonFloat(7) == 7, ComparisonComplex(7) == 7,
      ComparisonDict(a=1) == {"a": 1}, ComparisonSet([1]) == {1}, ComparisonFrozen([1]) == {1})
wide_integer = 2 ** 53 + 1
rounded_float = float(2 ** 53)
print("numeric-admission", int.__eq__(1, 1.0) is NotImplemented,
      float.__eq__(1.0, 1), complex.__lt__(1j, 2j) is NotImplemented)
print("numeric-precision", wide_integer == rounded_float, wide_integer > rounded_float,
      float.__eq__(rounded_float, wide_integer), float.__lt__(rounded_float, wide_integer),
      complex.__eq__(complex(rounded_float, 0), wide_integer))
print("numeric-extremes", 10 ** 500 < float("inf"), -(10 ** 500) > -float("inf"),
      float.__lt__(-1.5, -1), float.__gt__(1.5, 1), float.__eq__(-0.0, 0))
comparison_nan = float("nan")
print("numeric-unordered", float.__eq__(comparison_nan, comparison_nan),
      float.__ne__(comparison_nan, comparison_nan), float.__le__(comparison_nan, 0))

print("set-orders", {1} < {1, 2}, {1, 2} > frozenset({1}), {1} <= {2}, {1} >= {2})
print("set-view-reflection", set.__eq__({1}, {1: 0}.keys()) is NotImplemented,
      {1} == {1: 0}.keys(), {1: []}.items() == {1: []}.items(),
      {1: []}.items() < {1: [], 2: []}.items())
print("mixed-set-views", {(1, 2)} == {1: 2}.items(), {(1, 2): 0}.keys() == {1: 2}.items())
values_view = {1: 2}.values()
print("values-identity", values_view == values_view, values_view == {1: 2}.values())


class ReentrantComparisonKey:
    def __init__(self, kind):
        self.kind = kind
        self.target = None

    def __hash__(self):
        return 97

    def __eq__(self, other):
        target = self.target
        self.target = None
        if target is not None:
            target.clear()
            if self.kind == "dict":
                target[other] = [1]
            else:
                target.add(other)
        return True


for collection_kind in ("dict", "set"):
    reentrant_left = ReentrantComparisonKey(collection_kind)
    reentrant_right = ReentrantComparisonKey(collection_kind)
    if collection_kind == "dict":
        container_left = {reentrant_left: [1]}
        container_right = {reentrant_right: [1]}
    else:
        container_left = {reentrant_left}
        container_right = {reentrant_right}
    reentrant_right.target = container_right
    print("comparison-reentry", collection_kind, container_left == container_right,
          len(container_left), len(container_right), reentrant_right.target is None)

print("range-values", range(0) == range(10, 0, 2), range(5, 6) == range(5, 10, 10),
      range(0, 10, 3) == range(0, 11, 3),
      range(0, 10 ** 100, 2) == range(0, 10 ** 100 - 1, 2),
      range.__lt__(range(3), range(4)) is NotImplemented)
slice_order_result = object()


class SliceComparisonField:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        return slice_order_result


slice_left = slice(SliceComparisonField(), 5, 1)
slice_right = slice(SliceComparisonField(), 5, 1)
print("slice-fields", (slice_left < slice_right) is slice_order_result,
      slice(comparison_nan) == slice(comparison_nan), slice(1, 2) != slice(1, 3))
print("type-aliases", list[int] == list[int], list[int] != list[str],
      (int | str) == (str | int), (int | str) != (float | str))


class AliasComparisonResult:
    def __bool__(self):
        raise AssertionError("alias result must not be consumed as truth")


alias_comparison_result = AliasComparisonResult()


class AliasArguments(tuple):
    def __eq__(self, other):
        return alias_comparison_result


alias_left = GenericAlias(list, AliasArguments((int,)))
alias_right = GenericAlias(list, AliasArguments((str,)))
print("alias-value-result", (alias_left == alias_right) is alias_comparison_result,
      alias_left != alias_right, GenericAlias.__eq__(alias_left, alias_right) is alias_comparison_result)


def closure_factory():
    return lambda: 1
print("function-identity", closure_factory() == closure_factory())

buffer_integer = memoryview(array("q", [2 ** 60 + 1, -2 ** 60 - 1]))
buffer_unsigned = memoryview(array("Q", [2 ** 64 - 1]))
print("buffer-wide", buffer_integer == memoryview(array("q", [2 ** 60 + 1, -2 ** 60 - 1])),
      buffer_integer != memoryview(array("q", [2 ** 60, -2 ** 60 - 1])),
      buffer_unsigned == memoryview(array("Q", [2 ** 64 - 1])))
print("buffer-numeric", memoryview(array("i", [1, 2])) == array("d", [1.0, 2.0]),
      memoryview(array("q", [wide_integer])) == array("d", [rounded_float]))
buffer_nan = memoryview(array("d", [comparison_nan]))
print("buffer-nan", buffer_nan == buffer_nan, buffer_nan != buffer_nan)
buffer_shape = memoryview(b"abcd").cast("B", (2, 2))
print("buffer-layout", memoryview(b"abcd")[::-1] == b"dcba",
      buffer_shape == memoryview(b"abcd"), buffer_shape == memoryview(b"abcd").cast("B", (2, 2)))
released_left = memoryview(b"a")
released_right = memoryview(b"a")
released_left.release()
released_right.release()
print("buffer-released", released_left == released_left, released_left == released_right,
      released_left != released_right, memoryview.__eq__(memoryview(b"a"), object()) is NotImplemented)


for set_operation in ("contains", "add", "discard"):
    lookup_key = ReentrantComparisonKey("set")
    stored_key = ReentrantComparisonKey("set")
    lookup_target = {stored_key}
    stored_key.target = lookup_target
    if set_operation == "contains":
        lookup_outcome = lookup_key in lookup_target
    elif set_operation == "add":
        lookup_outcome = lookup_target.add(lookup_key)
    else:
        lookup_outcome = lookup_target.discard(lookup_key)
    print("set-lookup-reentry", set_operation, lookup_outcome, len(lookup_target))


class CachedComparisonHash:
    def __init__(self):
        self.armed = False

    def __hash__(self):
        if self.armed:
            raise AssertionError("set storage must reuse its cached hash")
        return 1234567


cached_hash_key = CachedComparisonHash()
cached_hash_set = {cached_hash_key}
cached_hash_key.armed = True
print("set-cached-hash", len(cached_hash_set.copy()), len(cached_hash_set | {0}),
      len(cached_hash_set & cached_hash_set), len(cached_hash_set - {0}),
      len(cached_hash_set ^ {0}), cached_hash_set.issubset(cached_hash_set),
      cached_hash_set.issuperset(cached_hash_set), cached_hash_set.isdisjoint({0}))
for update_name in ("update", "intersection_update", "difference_update", "symmetric_difference_update"):
    update_target = {0}
    getattr(update_target, update_name)(cached_hash_set)
    print("set-cached-update", update_name, len(update_target))


# View rich comparisons observe the other operand's protocols, including
# rehashing requested keys at the containment boundary.
view_protocol_events = []


class ViewProtocolSet(set):
    def __len__(self):
        view_protocol_events.append("len")
        return super().__len__()

    def __contains__(self, key):
        view_protocol_events.append("contains")
        return False

    def __iter__(self):
        view_protocol_events.append("iter")
        return iter((1,))


protocol_keys = {1: None}.keys()
protocol_set = ViewProtocolSet((99,))
print("view-protocol-eq", protocol_keys == protocol_set, view_protocol_events)
view_protocol_events.clear()
print("view-protocol-ge", protocol_keys >= protocol_set, view_protocol_events)
view_protocol_events.clear()
print("set-declaring-contains", set.__contains__(protocol_set, 99), view_protocol_events)
print("dict-view-contains-owner", type(type({}.keys()).__contains__).__name__,
      type(type({}.items()).__contains__).__name__)
for view_kind in ("keys", "items"):
    view_key = CachedComparisonHash()
    view_dict = {view_key: []}
    hash_view = getattr(view_dict, view_kind)()
    view_key.armed = True
    try:
        hash_view == hash_view
    except AssertionError:
        print("view-rehash", view_kind)


class ViewStoredValue:
    def __eq__(self, other):
        view_protocol_events.append("stored-value")
        return True


class ViewRequestedValue:
    def __eq__(self, other):
        raise AssertionError("dict item membership must compare stored value first")


view_protocol_events.clear()
print("view-item-value-order", (1, ViewRequestedValue()) in {1: ViewStoredValue()}.items(),
      view_protocol_events)
print("view-item-shape", [1, 2] in {1: 2}.items(), (1,) in {1: 2}.items())


def stop_sensitive_iter(first):
    yield first
    raise AssertionError("set operation consumed beyond its answer")


print("set-stream-stop", {1}.isdisjoint(stop_sensitive_iter(1)),
      {1}.issuperset(stop_sensitive_iter(2)),
      {1}.issubset(stop_sensitive_iter(1)),
      sorted({1}.intersection(stop_sensitive_iter(1))))
stream_update_target = {1}
stream_update_target.intersection_update(stop_sensitive_iter(1))
print("set-intersection-update-stream", sorted(stream_update_target))
view_protocol_events.clear()
print("set-subclass-iteration", {1}.isdisjoint(protocol_set), view_protocol_events)
multi_intersection_target = {1, 2}
try:
    multi_intersection_target.intersection_update({1}, stop_sensitive_iter(2))
except AssertionError:
    print("set-multi-intersection-atomic", sorted(multi_intersection_target))


class SetLateFailure:
    def __init__(self):
        self.armed = False

    def __hash__(self):
        return 1

    def __eq__(self, other):
        if self.armed:
            raise AssertionError("later comparison failed")
        return self is other


for mutation in ("difference_update", "symmetric_difference_update"):
    stored_failure = SetLateFailure()
    requested_failure = SetLateFailure()
    partial_target = {0, stored_failure}
    partial_source = {0, requested_failure}
    stored_failure.armed = True
    try:
        getattr(partial_target, mutation)(partial_source)
    except AssertionError:
        print("set-partial-mutation", mutation, 0 in partial_target, len(partial_target))

cached_dict_key = CachedComparisonHash()
cached_source_dict = {cached_dict_key: None}
cached_dict_key.armed = True
dict_update_target = set()
dict_update_target.update(cached_source_dict)
dict_symdiff_target = set()
dict_symdiff_target.symmetric_difference_update(cached_source_dict)
print("set-cached-dict", len(dict_update_target), len(dict_symdiff_target),
      len({0}.union(cached_source_dict)), len({0}.symmetric_difference(cached_source_dict)))


class FrozenCopySubclass(frozenset):
    pass


frozen_copy_source = FrozenCopySubclass((1,))
print("frozenset-subclass-copy", type(frozen_copy_source.copy()) is frozenset,
      frozen_copy_source.copy() is frozen_copy_source)


# Failed generic intersection retires iterator, result contents, then current
# key. These objects are uniquely owned, so destructor order is observable.
set_cleanup_events = []


class CleanupIntersectionKey:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        return other == 1

    def __del__(self):
        set_cleanup_events.append("result-key")


class CleanupIntersectionUnhashable(list):
    def __del__(self):
        set_cleanup_events.append("failing-key")


class CleanupIntersectionIterator:
    def __init__(self):
        self.index = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.index += 1
        if self.index == 1:
            return CleanupIntersectionKey()
        if self.index == 2:
            return CleanupIntersectionUnhashable()
        raise StopIteration

    def __del__(self):
        set_cleanup_events.append("iterator")


class CleanupIntersectionIterable:
    def __iter__(self):
        return CleanupIntersectionIterator()


try:
    {1, 2}.intersection(CleanupIntersectionIterable())
except TypeError:
    print("set-intersection-error-retirement", set_cleanup_events)
