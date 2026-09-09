"""Overflow peeling must not replay callbacks or expose a wrapped induction value."""


events = []


class Decision:
    def __init__(self, value):
        self.value = value

    def __bool__(self):
        events.append(("truth", self.value))
        return self.value


class Bound(int):
    def __gt__(self, other):
        events.append(("compare", other))
        return Decision(other < 9223372036854775810)


def compute(bound: int):
    index = 9223372036854775806
    while index < bound:
        index = index + 1
    return index


print(compute(Bound(9223372036854775810)))
print(events)
