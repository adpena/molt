"""Accumulators carried across an inner loop, and a product that overflows.

The outer accumulator crosses 2**63 inside the inner loop, so the overflow
transfer happens mid inner iteration and the rest of both loops run boxed.
The product crosses 2**63 in a body with a branch and a break.
"""


def nested(rows, cols):
    total = 0
    for r in range(rows):
        for c in range(cols):
            total += (r + 1) * (1 << 58) + c
        total += r
    return total


def product(n):
    acc = 1
    count = 0
    for k in range(1, n):
        if k % 2:
            acc *= k + 7
        else:
            acc *= 3
        count += 1
        if acc > 10**40:
            break
    return acc, count


def conditional_last_update(n):
    # The latch carries the accumulator unchanged on some paths, so its
    # phi cycle has no checked value on every incoming edge.
    total = 1 << 62
    for i in range(n):
        if i & 1:
            total += 1 << 61
    return total


print(nested(8, 9))
print(product(60))
print(conditional_last_update(20))
