"""Observers of a fused generator's frame see the generator's activation.

Purpose: a `for` loop that is the only consumer of a proven generator call
runs the generator's body in place, with no frame record on its hot path
(docs/design/foundation/spliced_frames.md). Each observer must still see the
generator's activation as CPython shows it: a traceback raised inside the
body lists the consumer at its `for` line, then the generator at its own line,
then any frame below; `tb_frame.f_locals` holds the generator's bindings; and
`sys._getframe()` inside the body returns the generator's frame, whose
`f_back` is the consumer.
"""

import sys


def stepped(n, k):
    for i in range(n):
        yield i * k


def divides(n, d):
    for i in range(n):
        yield 10 // (d - i)


def helper(x):
    return 100 // x


def calls(n):
    for i in range(n):
        yield helper(n - 1 - i)


def raiser(n):
    for i in range(n):
        if i == 2:
            raise KeyError(i)
        yield i


def observed(n):
    for i in range(n):
        yield (
            sys._getframe().f_code.co_name,
            sys._getframe().f_lineno,
            sys._getframe(1).f_code.co_name,
        )


def describe(exc):
    """Each traceback entry: function, line and the bindings a frame holds."""
    rows = []
    tb = exc.__traceback__
    while tb is not None:
        frame = tb.tb_frame
        bindings = {
            name: value
            for name, value in frame.f_locals.items()
            if name in ("n", "k", "d", "i", "x")
        }
        rows.append((frame.f_code.co_name, tb.tb_lineno, sorted(bindings.items())))
        tb = tb.tb_next
    return type(exc).__name__, rows


def bad_range():
    total = 0
    try:
        for v in stepped("x", 2):
            total += v
    except TypeError as exc:
        return total, describe(exc)
    return total, None


def zero_division():
    total = 0
    try:
        for v in divides(6, 3):
            total += v
    except ZeroDivisionError as exc:
        return total, describe(exc)
    return total, None


def below_the_generator():
    seen = []
    try:
        for v in calls(4):
            seen.append(v)
    except ZeroDivisionError as exc:
        return seen, describe(exc)
    return seen, None


def explicit_raise():
    seen = []
    try:
        for v in raiser(5):
            seen.append(v)
    except KeyError as exc:
        return seen, describe(exc)
    return seen, None


def introspection():
    rows = []
    for row in observed(2):
        rows.append(row)
    return rows


def consumer_name_after_fusion():
    # The consumer's own frame reads its own name and line after the loop.
    total = 0
    for v in stepped(3, 1):
        total += v
    return total, sys._getframe().f_code.co_name


def main():
    print(bad_range())
    print(zero_division())
    print(below_the_generator())
    print(explicit_raise())
    print(introspection())
    print(consumer_name_after_fusion())
    sys.stdout.flush()


main()
