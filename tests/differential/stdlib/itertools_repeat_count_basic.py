"""Purpose: differential coverage for itertools.repeat/count."""

import itertools

print(list(itertools.islice(itertools.repeat("x"), 3)))
print(list(itertools.islice(itertools.count(5, 2), 3)))

# Native declarations must bind through ordinary instance attribute access.
for iterator in [itertools.repeat("bound", 2), itertools.count(7, 3)]:
    print(iterator.__iter__() is iterator)
    print(iterator.__next__(), iterator.__next__())

# __new__ remains explicit-self and distinguishes omission from explicit None.
print(list(itertools.islice(itertools.repeat.__new__(itertools.repeat, "new"), 3)))
for times in [0, -2, 2, False, True, None, 1.5]:
    try:
        print(list(itertools.repeat("value", times)))
    except Exception as exc:
        print(type(exc).__name__)

class RepeatCount:
    def __init__(self):
        self.calls = 0

    def __index__(self):
        self.calls += 1
        return 2

count = RepeatCount()
print(list(itertools.repeat("indexed", count)), count.calls)

class RepeatCountError:
    def __index__(self):
        raise ValueError("repeat count error")

try:
    itertools.repeat("value", RepeatCountError())
except Exception as exc:
    print(type(exc).__name__, str(exc))
