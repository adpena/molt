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
