"""The iteration that overflows also raises, inside a try around the loop.

The boxed loop finishes the overflowing iteration; its exception reaches the
handler outside the loop with the exact accumulator.
"""


def run(n, limit):
    total = (1 << 63) - 50
    i = 0
    try:
        while i < n:
            total += 7
            if total > limit:
                raise ValueError(total)
            i += 1
    except ValueError as exc:
        return "raised", exc.args[0], i, total
    return "done", total, i


print(run(100, (1 << 63) + 20))
print(run(3, 1 << 70))
