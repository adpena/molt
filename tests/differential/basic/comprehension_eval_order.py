"""Purpose: differential coverage for comprehension evaluation order."""

events = []


def gen(label, n):
    for i in range(n):
        events.append(f"iter:{label}{i}")
        yield i


def guard(label, value):
    events.append(f"guard:{label}{value}")
    return True


vals = [
    (i, j)
    for i in gen("a", 2)
    if guard("i", i)
    for j in gen("b", 2)
    if guard("j", j)
]

print("vals", vals)
print("events", events)
