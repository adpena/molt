from _collections import _count_elements


class CountKey:
    def __init__(self):
        self.hash_calls = 0

    def __hash__(self):
        self.hash_calls += 1
        return 71


for mapping_type in (dict,):
    key = CountKey()
    mapping = mapping_type()
    result = _count_elements(mapping, [key, key])
    print("count-fast", result is None, key.hash_calls, list(mapping.values()))


class InheritedDict(dict):
    pass


class AliasedDict(dict):
    get = dict.get
    __setitem__ = dict.__setitem__


for mapping_type in (InheritedDict, AliasedDict):
    key = CountKey()
    mapping = mapping_type()
    mapping.get = lambda *args: (_ for _ in ()).throw(AssertionError("instance shadow"))
    _count_elements(mapping, [key, key])
    print("count-inherited", key.hash_calls, list(mapping.values()))

count_events = []


class OverriddenDict(dict):
    def get(self, key, default):
        count_events.append("get")
        return dict.get(self, key, default)

    def __setitem__(self, key, value):
        count_events.append("set")
        dict.__setitem__(self, key, value)


key = CountKey()
mapping = OverriddenDict()
_count_elements(mapping, [key, key])
print("count-overrides", key.hash_calls, list(mapping.values()), count_events)


class ArbitraryCount:
    def __init__(self, mapping):
        self.mapping = mapping

    def __add__(self, one):
        self.mapping.clear()
        return ("sum", one)


mapping = {}
mapping["key"] = ArbitraryCount(mapping)
_count_elements(mapping, ["key"])
print("count-reentry", mapping)
mapping = {"wide": 2 ** 100}
_count_elements(mapping, ["wide"])
print("count-wide", mapping["wide"] == 2 ** 100 + 1)


class CountMapping:
    def __init__(self):
        self.data = {}

    def get(self, key, default):
        count_events.append(("get", key, default))
        return self.data.get(key, default)

    def __setitem__(self, key, value):
        count_events.append(("set", key, value))
        self.data[key] = value


count_events.clear()
mapping = CountMapping()
_count_elements(mapping, ["a", "a", "b"])
print("count-mapping", mapping.data, count_events)


def failing_counts():
    yield "first"
    raise ValueError("later item")


mapping = {}
try:
    _count_elements(mapping, failing_counts())
except ValueError:
    print("count-partial", mapping)


class BrokenGet:
    @property
    def get(self):
        raise ValueError("get binding")


try:
    _count_elements(BrokenGet(), ())
except ValueError as error:
    print("count-empty-binding", str(error))

count_events.clear()


class OrderedIterable:
    def __iter__(self):
        count_events.append("iter")
        return iter(())


class OrderedMapping:
    @property
    def get(self):
        count_events.append("get-binding")
        return lambda key, default: default


_count_elements(OrderedMapping(), OrderedIterable())
print("count-acquisition-order", count_events)


# The mapping cannot assign, so the native protocol raises without retaining
# current key/value in a Python __setitem__ traceback frame.
count_cleanup_events = []


class CleanupCountKey:
    def __del__(self):
        count_cleanup_events.append("key")


class CleanupCountValue:
    def __del__(self):
        count_cleanup_events.append("new-value")


class CleanupCountOld:
    def __add__(self, one):
        return CleanupCountValue()


class CleanupCountGet:
    def __call__(self, key, default):
        return CleanupCountOld()

    def __del__(self):
        count_cleanup_events.append("get")


class CleanupCountMapping:
    @property
    def get(self):
        return CleanupCountGet()


class CleanupCountIterator:
    def __init__(self, empty):
        self.empty = empty

    def __iter__(self):
        return self

    def __next__(self):
        if self.empty:
            raise StopIteration
        self.empty = True
        return CleanupCountKey()

    def __del__(self):
        count_cleanup_events.append("iterator")


class CleanupCountIterable:
    def __init__(self, empty):
        self.empty = empty

    def __iter__(self):
        return CleanupCountIterator(self.empty)


try:
    _count_elements(CleanupCountMapping(), CleanupCountIterable(False))
except TypeError:
    print("count-error-retirement", count_cleanup_events)
count_cleanup_events.clear()
_count_elements(CleanupCountMapping(), CleanupCountIterable(True))
print("count-empty-retirement", count_cleanup_events)
