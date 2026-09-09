"""Conditional increments preserve callbacks and the untaken path's identity."""

events = []


class Counter(int):
    def __add__(self, other):
        events.append(("add", type(other).__name__, other))
        return Counter(int(self) + int(other))

    def __iadd__(self, other):
        events.append(("iadd", type(other).__name__, other))
        return Counter(int(self) + int(other))


class Condition:
    def __init__(self, value):
        self.value = value

    def __bool__(self):
        events.append(("truth", self.value))
        return self.value


def increment(count: int, condition: bool):
    if condition:
        count = count + 1
    return count


def inplace_increment(count: int, condition: bool):
    if condition:
        count += 1
    return count


for operation in (increment, inplace_increment):
    for selected in (False, True):
        original = Counter(12)
        events.clear()
        result = operation(original, Condition(selected))
        print(operation.__name__, selected, int(result), result is original, events)

large = int("123456789123456789123456789")
print("untaken-bigint", increment(large, False) is large)
print("taken-bigint", increment(large, True) == large + 1)
