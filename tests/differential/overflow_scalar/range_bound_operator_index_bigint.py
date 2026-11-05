"""Purpose: range() bounds converted by operator.index stay exact big ints.

The compiler types an operator.index result as an exact int of any magnitude
(call_anatomy.md, "Location publication cost", M4). A counted loop over such
bounds must compare, step and bind big ints exactly, never as truncated i64.
"""

import sys


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


def head(start, stop, step):
    seen = []
    for i in range(start, stop, step):
        seen.append(i)
        if len(seen) == 3:
            break
    return seen


def count_to(stop):
    total = 0
    for i in range(stop):
        total += i
    return total


big = (1 << 70) + len(sys.argv)
print(head(big, big + 10, 3))
print(head(Index(big), Index(big + 5), 1))
print(head(-big, -big - 6, -2))
print(head(0, big, big // 3))
print(head(big - 1, big + 1, 1))
print(head((1 << 63) - 2, (1 << 63) + 2, 1))
print(count_to(Index(5)), count_to(True), count_to(-3))
