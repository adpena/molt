"""An accumulator in a branching loop body crosses 2**63 mid-iteration.

The checked fast loop must hand the exact sum to the boxed loop at the
overflowing operation and finish that iteration's later statements there:
the second update, the list append and the print all see exact values.
"""

STEP = 1 << 61


def run(n):
    total = 0
    seen = []
    for i in range(n):
        if i % 3 == 0:
            total += STEP
        else:
            total += 1
        total += i
        if i % 4 == 0:
            seen.append(total)
        if total > (1 << 63) and len(seen) < 100:
            print("past 2**63 at", i, total)
            seen.append(-1)
    return total, seen


total, seen = run(40)
print(total)
print(seen)
print(type(total).__name__)
