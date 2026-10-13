"""Purpose: differential coverage for functools.lru_cache eviction and cache_info."""

from functools import lru_cache


@lru_cache(maxsize=2)
def f(x):
    return x * 10


print("v", f(1), f(2), f(3))
print("v2", f(2), f(1))
info = f.cache_info()
print("info", info.hits, info.misses, info.maxsize, info.currsize)


# A nested bounded call publishes first; the outer invocation still returns its
# own result. Unlimited caches deliberately retain the last computed result.
for limit in (None, 1, 2):
    active = False
    computed = []

    @lru_cache(maxsize=limit)
    def recursive(key):
        global active
        if active:
            computed.append("inner")
            return "inner"
        active = True
        try:
            recursive(key)
        finally:
            active = False
        computed.append("outer")
        return "outer"

    print("recursive", limit, recursive(1), recursive(1), computed)
    recursive(2)
    print("recursive size", limit, recursive.cache_info().currsize)
    recursive.cache_clear()


# A successful call hashes its key once. Neither publication nor eviction may
# replay a user hash after the function has invalidated it.
class SingleHashKey:
    def __init__(self, label):
        self.label = label
        self.calls = 0

    def __hash__(self):
        self.calls += 1
        if self.calls != 1:
            raise AssertionError("cache replayed hash")
        return 13

    def __eq__(self, other):
        return self is other


for limit in (None, 1):

    @lru_cache(maxsize=limit)
    def once(key):
        return key.label

    first = SingleHashKey("first")
    second = SingleHashKey("second")
    print("one hash", limit, once(first), once(second), first.calls, second.calls)
    once.cache_clear()


# Equality may clear and repopulate the cache, then either return or raise.
# Both the fresh publication and the original exception must survive.
equality_error = ValueError("cache equality sentinel")


class ReentrantKey:
    def __init__(self, label):
        self.label = label
        self.action = None

    def __hash__(self):
        return 17

    def __eq__(self, other):
        action = self.action
        self.action = None
        if action is not None:
            action()
        return self is other


for raises in (False, True):
    calls = []

    @lru_cache(maxsize=2)
    def reentrant(key):
        calls.append(key.label)
        return key.label

    resident = ReentrantKey("resident")
    query = ReentrantKey("query")
    fresh = ReentrantKey("fresh")
    reentrant(resident)

    def replace():
        reentrant.cache_clear()
        reentrant(fresh)
        if raises:
            raise equality_error

    resident.action = replace
    try:
        print("equality result", raises, reentrant(query))
    except ValueError as exc:
        print("equality error", raises, exc is equality_error)
    print("equality publication", raises, reentrant(fresh), calls)
    print("equality bounded", raises, reentrant.cache_info().currsize <= 2)
    reentrant.cache_clear()


# Destructors run only after dictionary, order and counters agree. A newly
# published entry must survive an outer clear or eviction.
for operation in ("clear", "evict"):
    finalized = []
    made = []

    class RetiringValue:
        def __del__(self):
            finalized.append("victim")
            finalizing("revived")

    @lru_cache(maxsize=1)
    def finalizing(key):
        made.append(key)
        return RetiringValue() if key == "victim" else key

    finalizing("victim")
    if operation == "clear":
        finalizing.cache_clear()
    else:
        finalizing("other")
    print("finalizer", operation, finalized, made, finalizing("revived"))
    print("finalizer size", operation, finalizing.cache_info().currsize)
    finalizing("last")
    print("finalizer bounded", operation, finalizing.cache_info().currsize)
    finalizing.cache_clear()


# Recency follows equal keys' canonical entries, and retired entries release
# their key owner exactly once. A long hit-only streak does not change the LRU.
retired = []


class MortalKey:
    def __init__(self, label):
        self.label = label

    def __hash__(self):
        return 29

    def __eq__(self, other):
        return isinstance(other, MortalKey) and self.label == other.label

    def __del__(self):
        retired.append(self.label)


@lru_cache(maxsize=2)
def retaining(key):
    return key.label


retaining(MortalKey("first"))
retaining(MortalKey("second"))
for _ in range(64):
    retaining(MortalKey("first"))
print("equal query retirement", retired.count("first"), retired.count("second"))
retaining(MortalKey("third"))
print("oldest retired", retired.count("first"), retired.count("second"))
retaining.cache_clear()
print(
    "all retired",
    retired.count("first"),
    retired.count("second"),
    retired.count("third"),
)


# Bounded misses expose the post-call lookup and the final publication probe.
# An exception from the latter must escape unchanged without publishing a row.
publication_error = ValueError("publication comparison")
comparisons = []
computed = []


class PublicationKey:
    def __init__(self, label):
        self.label = label

    def __hash__(self):
        return 17

    def __eq__(self, other):
        comparisons.append((self.label, other.label))
        if len(comparisons) == 3:
            raise publication_error
        return self is other


@lru_cache(maxsize=4)
def publication(key):
    computed.append(key.label)
    return key.label


publication(PublicationKey("resident"))
try:
    publication(PublicationKey("query"))
except ValueError as exc:
    print("publication failure", exc is publication_error, len(comparisons), computed)
else:
    print("publication failure missing")
print("publication retained size", publication.cache_info().currsize)
publication.cache_clear()


# Clear retires results in current LRU order, which differs from insertion order.
clear_order = []


class OrderedKey:
    def __init__(self, label):
        self.label = label

    def __hash__(self):
        return 17

    def __eq__(self, other):
        return isinstance(other, OrderedKey) and self.label == other.label


class OrderedResult:
    def __init__(self, key):
        self.key = key

    def __del__(self):
        clear_order.append(self.key.label)


@lru_cache(maxsize=3)
def ordered_clear(key):
    return OrderedResult(key)


for key in (1, 2, 3, 1):
    ordered_clear(OrderedKey(key))
ordered_clear.cache_clear()
print("clear touched order", clear_order, ordered_clear.cache_info().currsize)


# Exact single int/str keys are distinct from bool/float/subclass tuple keys.
class IntSubclass(int):
    pass


class StrSubclass(str):
    pass


for typed in (False, True):
    for limit in (None, 16):
        key_calls = []

        @lru_cache(maxsize=limit, typed=typed)
        def key_identity(*args, **kwargs):
            key_calls.append((tuple(type(x).__name__ for x in args), tuple(kwargs)))
            return len(key_calls)

        print(
            "key classes",
            typed,
            limit,
            [
                key_identity(x)
                for x in (
                    1,
                    True,
                    1.0,
                    IntSubclass(1),
                    0,
                    0.0,
                    False,
                    "x",
                    StrSubclass("x"),
                    "x",
                )
            ],
        )
        print("empty kwargs", key_identity(1), key_identity(1, **{}))
        print("two args", key_identity(1, 2), key_identity(True, 2.0))
        print("keyword order", key_identity(a=1, b=2), key_identity(b=2, a=1))
        print("key cache info", key_identity.cache_info())
        key_identity.cache_clear()


# Native cache payload and the ordinary instance dictionary have separate owners.
for limit in (0, 2, None):

    @lru_cache(maxsize=limit)
    def attributed(value):
        return value + 10

    marker = []
    attributed.payload = marker
    info_type = type(attributed.cache_info())
    print(
        "wrapper storage",
        limit,
        attributed.payload is marker,
        attributed.__dict__["payload"] is marker,
        attributed.cache_parameters(),
        attributed.__wrapped__(5),
        attributed(5),
    )
    del attributed.payload
    print(
        "wrapper storage retained",
        type(attributed.cache_info()) is info_type,
        "payload" in attributed.__dict__,
    )
    attributed.cache_clear()
