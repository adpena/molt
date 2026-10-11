"""ChainMap's public mappings own identity, protocol dispatch, and lifetime."""

from collections import ChainMap, UserDict, defaultdict
from collections.abc import Hashable, MutableMapping
import copy
import gc


def outcome(label, operation):
    try:
        print(label, "ok", operation())
    except Exception as error:
        print(label, type(error).__name__, str(error))


first = {"first": 1, "shared": "front"}
second = {"second": 2, "shared": "back"}
chain = ChainMap(first, second)
maps = chain.maps
print("public storage", maps is chain.maps, maps[0] is first, maps[1] is second)
print("mapping", isinstance(chain, MutableMapping), list(chain), dict(chain))
keys = chain.keys()
values = chain.values()
items = chain.items()
maps.insert(0, {"new": 3})
print("live views", list(keys), list(values), list(items))
chain.maps = [UserDict({"user": 4}), second]
chain["write"] = 5
print("replaced storage", chain["user"], chain.maps[0]["write"], dict(chain))
print("old public list", maps is chain.maps, len(maps))


class Missing(ChainMap):
    def __missing__(self, key):
        return ("missing", key)


factory_calls = []


def factory():
    factory_calls.append("factory")
    return 17


defaults = defaultdict(factory)
wrapped = Missing(defaults, {"later": 9})
print("get skips missing", wrapped.get("absent", 23), factory_calls)
print("getitem uses mapping", wrapped["absent"], factory_calls)
print("subclass missing", Missing({})["absent"])


class ExplicitMapping(MutableMapping):
    def __init__(self, values):
        self.data = dict(values)

    def __getitem__(self, key):
        return self.data[key]

    def __setitem__(self, key, value):
        self.data[key] = value

    def __delitem__(self, key):
        del self.data[key]

    def __iter__(self):
        return iter(self.data)

    def __len__(self):
        return len(self.data)


print("explicit mapping", dict(ChainMap(ExplicitMapping({"m": 7}))))
print("explicit union", dict(ChainMap({"a": 1}) | ExplicitMapping({"b": 2})))
print(
    "UserDict membership",
    isinstance(UserDict(), MutableMapping),
    isinstance(UserDict(), Hashable),
)
generic = ChainMap(UserDict({"a": 1}), UserDict({"b": 2}))
print("generic mappings", list(generic), dict(generic), generic == {"a": 1, "b": 2})
print(
    "UserDict equality",
    generic == UserDict({"a": 1, "b": 2}),
    UserDict({"a": 1, "b": 2}) == generic,
)
unvalidated = ChainMap(5)
print("construction defers protocol", unvalidated.maps)
outcome("invalid mapping lookup", lambda: unvalidated["a"])

base = Missing({"a": 1}, {"b": 2})
cloned = copy.copy(base)
print(
    "copy",
    type(cloned).__name__,
    cloned.maps[0] is base.maps[0],
    cloned.maps[1] is base.maps[1],
)
new_front = UserDict({"c": 3})
child = base.new_child(new_front, d=4)
print(
    "child",
    type(child).__name__,
    child.maps[0] is new_front,
    child.maps[1] is base.maps[0],
    dict(new_front),
)
print("parents", type(child.parents).__name__, child.parents.maps[0] is base.maps[0])
print("single parents", ChainMap({"x": 1}).parents.maps)


class UserSubclass(UserDict):
    pass


submap = UserSubclass({"value": 8})
submap.marker = object()
subcopy = ChainMap(submap, second).copy()
print(
    "UserDict subclass copy",
    type(subcopy.maps[0]).__name__,
    subcopy.maps[0] is submap,
    subcopy.maps[0].data is submap.data,
    subcopy.maps[0].marker is submap.marker,
    subcopy.maps[1] is second,
    subcopy["value"],
)

print("fromkeys", dict(Missing.fromkeys(["z", "z", "y"], 6)))
outcome("fromkeys iterable keyword", lambda: dict(ChainMap.fromkeys(iterable=["k"])))
outcome("fromkeys value keyword", lambda: dict(ChainMap.fromkeys(["k"], value=1)))

left = ChainMap({"a": 1}, {"b": 2, "a": 0})
right = left | UserDict({"c": 3, "a": 4})
reflected = {"z": 0, "a": 8} | left
print(
    "unions",
    dict(right),
    right.maps[1] is left.maps[1],
    dict(reflected),
    len(reflected.maps),
)
original = left
left |= [("d", 5)]
print("inplace union", left is original, left.maps)

mutated = ChainMap({"front": 1}, {"tail": 2})
outcome("pop tail forbidden", lambda: mutated.pop("tail"))
outcome("delete tail forbidden", lambda: mutated.__delitem__("tail"))
print("pop default", mutated.pop("missing", 8))
print("popitem first", mutated.popitem())
outcome("popitem empty first", mutated.popitem)
mutated["fresh"] = 9
mutated.clear()
print("clear only first", mutated.maps)
mutated.maps.clear()
print("empty public storage", len(mutated), bool(mutated), list(mutated))
outcome("empty write", lambda: mutated.__setitem__("x", 1))

events = []


class Payload:
    def __del__(self):
        events.append("payload retired")


owned = ChainMap({"payload": Payload()})
gc.collect()
print("temporary map retained", events)
value = owned["payload"]
owned.maps[0].clear()
gc.collect()
print("returned value retained", events)
del value
gc.collect()
print("returned value released", events)


class ReentrantMap(dict):
    chain = None

    def __getitem__(self, key):
        self.chain.maps[:] = [{"key": "replacement"}]
        raise KeyError(key)


reentrant = ReentrantMap()
reentered = ChainMap(reentrant, {"key": "old tail"})
reentrant.chain = reentered
outcome("reentrant current traversal", lambda: reentered["key"])
print("reentrant later traversal", reentered["key"])

sentinel = ValueError("mapping callback sentinel")


class BrokenMap(dict):
    def __getitem__(self, key):
        raise sentinel

    def __contains__(self, key):
        raise sentinel


broken = ChainMap(BrokenMap(), {"key": 1})
for label, operation in [
    ("lookup", lambda: broken["key"]),
    ("contains", lambda: "key" in broken),
    ("get", lambda: broken.get("key")),
]:
    try:
        operation()
    except ValueError as error:
        print("callback identity", label, error is sentinel)


class StopsContainment(dict):
    def __contains__(self, key):
        raise StopIteration("contains stopped")


try:
    "key" in ChainMap(StopsContainment())
except Exception as error:
    cause = error.__cause__
    print(
        "versioned containment",
        type(error).__name__,
        type(cause).__name__ if cause else None,
    )

recursive = ChainMap({})
recursive.maps[0]["self"] = recursive
print("recursive repr", repr(recursive))
recursive.maps[0].clear()
outcome("mapping unhashable", lambda: hash(ChainMap()))
