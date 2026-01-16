import functools


def cmp(a, b):
    return (a > b) - (a < b)


nums = [5, 1, 4, 2]
print(sorted(nums, key=functools.cmp_to_key(cmp)))


@functools.total_ordering
class Box:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return self.value == other.value

    def __lt__(self, other):
        return self.value < other.value


print(Box(1) < Box(2), Box(2) > Box(1))
print(Box(1) <= Box(1), Box(2) >= Box(1))
