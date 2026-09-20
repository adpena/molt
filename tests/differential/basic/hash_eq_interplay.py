"""Purpose: differential coverage for hash eq interplay."""


class Key:
    def __init__(self, value: int) -> None:
        self.value = value

    def __hash__(self) -> int:
        return 1

    def __eq__(self, other) -> bool:
        return isinstance(other, Key) and self.value == other.value


k1 = Key(1)
k2 = Key(2)

store = {k1: "a", k2: "b"}
print(len(store), store[k1], store[k2])

k1.value = 3
print(k1 in store)


class TracedKey:
    def __init__(self, value, failure, events):
        self.value = value
        self.failure = failure
        self.events = events

    def __hash__(self):
        self.events.append(("hash", self.value))
        if self.failure == "hash" + str(self.value):
            raise ValueError("hash failure")
        return 1

    def __eq__(self, other):
        self.events.append(("eq", self.value, other.value))
        if self.failure == "eq":
            raise ValueError("equality failure")
        return self.value == other.value


def construct(kind, keys):
    if kind == "dict":
        return {keys[0]: "a", keys[1]: "b", keys[2]: "c"}
    if kind == "set":
        return {keys[0], keys[1], keys[2]}
    return frozenset(keys)


# A failed insertion must stop later hash/equality calls, preserve the first
# exception, and never expose a partially initialized container as a result.
for kind in ("dict", "set", "frozenset"):
    for failure in ("none", "hash0", "hash1", "eq"):
        events = []
        keys = [TracedKey(i, failure, events) for i in range(3)]
        try:
            result = construct(kind, keys)
            print(kind, failure, "size", len(result))
        except ValueError as error:
            print(kind, failure, str(error))
        print(events)
