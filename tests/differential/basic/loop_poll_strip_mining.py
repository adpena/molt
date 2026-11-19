"""Counted loops whose exit test carries the back-edge poll.

The loop tests its counter against a chunk limit of 1024 and re-tests the
real bound at each chunk boundary. Each case sits on an edge of that scheme:
lengths around the chunk size, a counter that crosses the refill threshold
near the inline-int window's top, steps above one, early exits at a
boundary, and an inner loop entered many times.
"""

CHUNK = 1024


def total(start, stop, step=1):
    acc = 0
    for i in range(start, stop, step):
        acc += i
    return acc


def count(start, stop):
    n = 0
    for _ in range(start, stop):
        n += 1
    return n


for length in (0, 1, 2, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 3 * CHUNK + 5):
    print("length", length, count(0, length), total(0, length))

# Empty and reversed bounds never enter the body.
print("empty", total(10, 10), total(10, 5), count(-3, -7))

# Negative and mixed-sign ranges.
print("negative", total(-5000, -1000), total(-3000, 3000), count(-2 * CHUNK, CHUNK))

# Steps above one.
for step in (2, 3, CHUNK, CHUNK + 1):
    print("step", step, total(0, 10_000, step), total(-777, 9_999, step))

# The top of the inline-int47 window: the refill threshold is 2**46 - 1 - 1024.
top = 2**46 - 1
for start in (top - 3000, top - CHUNK - 5, top - CHUNK, top - 10):
    print("top", top - start, count(start, top), total(start, top) - (top - start) * start)
# Bounds past the window take the general loop.
print("past", count(top - 5, top + 5), total(2**63 - 5, 2**63 - 1) - 4 * (2**63 - 5))


def first_at(stop, target):
    for i in range(stop):
        if i == target:
            break
    else:
        return ("else", stop)
    return ("break", i)


for target in (CHUNK - 1, CHUNK, CHUNK + 1, 5 * CHUNK):
    print("break", target, first_at(4 * CHUNK, target))


def raise_at(stop, target):
    seen = 0
    try:
        for i in range(stop):
            seen += 1
            if i == target:
                raise ValueError(i)
    except ValueError as exc:
        return ("raised", exc.args[0], seen)
    return ("done", seen)


for target in (0, CHUNK - 1, CHUNK, 2 * CHUNK, 10 * CHUNK):
    print("raise", target, raise_at(3 * CHUNK, target))


def nested(outer, inner):
    acc = 0
    for i in range(outer):
        for j in range(inner):
            acc += i ^ j
    return acc


print("nested", nested(3000, 3), nested(5, CHUNK + 3), nested(0, 9), nested(9, 0))
