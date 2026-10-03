"""Purpose: differential coverage for container mutation."""


def section(name):
    print(f"--- {name} ---")


section("Bytearray Slice Assignment")
b = bytearray(b"0123456789")
print(b)
b[2:5] = b"abc"  # Replace 3 chars with 3
print(b)
b[2:5] = b"XYZ"  # Replace 3 with 3
print(b)
b[2:5] = b"M"  # Replace 3 with 1 (shrink)
print(b)
b[2:3] = b"LONG"  # Replace 1 with 4 (grow)
print(b)

section("List Self-Assignment")
lst = [0, 1, 2, 3, 4]
lst[:] = lst  # Should be safe
print(lst)
lst[1:4] = lst[1:4]
print(lst)
# Tricky: lst[1:4] = lst (assigning whole list to slice)
lst[1:4] = lst
print(len(lst))  # 2 (0) + 5 (inserted) + 1 (4) = 8?

section("Set Self-Update")
s = {1, 2, 3}
s |= s
print(s)
s &= s
print(s)
s -= s
print(s)
s ^= {1, 2}  # {1, 2}
print(s)
s ^= s
print(s)


section("Indexed Mutation Preserves Container Ownership")


def build_mapping() -> dict[str, int]:
    result: dict[str, int] = {}
    for index in range(4):
        result[str(index)] = index
    del result["1"]
    result["0"] = 10
    return result


def build_sequence() -> list[int]:
    result: list[int] = [0, 1, 2, 3]
    for index in range(4):
        result[index] = index + 10
    del result[1]
    return result


def mutate_unknown(container, key, value):
    container[key] = value
    del container[key]
    return container


mapping = build_mapping()
sequence = build_sequence()
print(mapping, sequence)
print(mutate_unknown(mapping, "extra", 99) is mapping, mapping)
print(mutate_unknown(sequence, -1, 99) is sequence, sequence)


class MutationResult:
    def __del__(self):
        events.append("result released")


class CustomContainer:
    def __setitem__(self, key, value):
        events.append(("set", key, value))
        return MutationResult()

    def __delitem__(self, key):
        events.append(("delete", key))
        return MutationResult()


events = []
custom = CustomContainer()
print(mutate_unknown(custom, "key", 7) is custom)
print(events)

section("Inline List Promotion And Callback Indexing")


def promoted_owners():
    owner = int("4611686018427387904")
    values = [0] * 4
    alias = values
    alias[1] = owner
    alias.append(owner)
    copied = values.copy()
    sliced = values[1::3]
    repeated = values * 2
    return (values[1] is owner, values[-1] is owner, copied[1] is owner,
            sliced[0] is owner, repeated[6] is owner, values[0], len(values))


print(promoted_owners())


class PromoteIndex:
    def __init__(self, values, owner, fail=False):
        self.values = values
        self.owner = owner
        self.fail = fail

    def __index__(self):
        self.values[0] = self.owner
        self.values.append(self.owner)
        if self.fail:
            raise ValueError("index callback")
        return 0


def callback_indexing():
    owner = int("4611686018427387904")
    for initial in (0, False):
        values = [initial] * 2
        read = values[PromoteIndex(values, owner)]
        print("callback read", read is owner, len(values), values[-1] is owner)
        values = [initial] * 2
        values[PromoteIndex(values, owner)] = owner
        print("callback store", values[0] is owner, len(values))
        values = [initial] * 2
        selected = values[slice(PromoteIndex(values, owner), None, None)]
        print("callback slice", len(selected), selected[0] is owner, selected[-1] is owner)
        values = [initial] * 2
        values[slice(PromoteIndex(values, owner), None, None)] = [owner]
        print("callback slice store", len(values), values[0] is owner)
        values = [initial] * 2
        del values[slice(PromoteIndex(values, owner), None, None)]
        print("callback slice delete", len(values))
        values = [initial] * 2
        try:
            values[PromoteIndex(values, owner, True)]
        except ValueError as error:
            print("callback error", str(error), len(values), values[0] is owner)


callback_indexing()
