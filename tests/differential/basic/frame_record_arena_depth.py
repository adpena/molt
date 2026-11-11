"""Purpose: deep recursion with wide frames keeps every frame's bindings, traceback and f_back chain intact."""

import sys


def wide(depth, limit):
    a0 = depth
    a1 = a0 + 1
    a2 = a1 + 1
    a3 = a2 + 1
    a4 = a3 + 1
    a5 = a4 + 1
    a6 = a5 + 1
    a7 = a6 + 1
    a8 = a7 + 1
    a9 = a8 + 1
    b0 = a9 + 1
    b1 = b0 + 1
    b2 = b1 + 1
    b3 = b2 + 1
    b4 = b3 + 1
    b5 = b4 + 1
    b6 = b5 + 1
    b7 = b6 + 1
    b8 = b7 + 1
    b9 = b8 + 1
    c0 = b9 + 1
    c1 = c0 + 1
    c2 = c1 + 1
    c3 = c2 + 1
    c4 = c3 + 1
    c5 = c4 + 1
    c6 = c5 + 1
    c7 = c6 + 1
    c8 = c7 + 1
    c9 = c8 + 1
    d0 = c9 + 1
    d1 = d0 + 1
    d2 = d1 + 1
    d3 = d2 + 1
    d4 = d3 + 1
    d5 = d4 + 1
    d6 = d5 + 1
    d7 = d6 + 1
    if depth == limit:
        frame = sys._getframe(0)
        seen = 0
        while frame is not None and frame.f_code.co_name == "wide":
            seen += 1
            frame = frame.f_back
        raise RuntimeError(f"bottom at {depth}, {seen} wide frames")
    result = wide(depth + 1, limit)
    # Every binding survived the callee's frames.
    return result + (d7 - a0 - 37)


def walk(limit):
    try:
        wide(0, limit)
    except RuntimeError as exc:
        depth = 0
        tb = exc.__traceback__
        while tb is not None:
            depth += 1
            last = tb.tb_frame.f_locals.get("depth")
            tb = tb.tb_next
        print(exc, "traceback entries", depth, "innermost depth", last)


def loop_on_one_depth(depth, calls):
    # Calls that return to the same depth again and again.
    if depth:
        return loop_on_one_depth(depth - 1, calls)
    total = 0
    for index in range(calls):
        total += leaf(index)
    return total


def leaf(value):
    local = value + 1
    return local - value


if __name__ == "__main__":
    for limit in (5, 120, 450):
        walk(limit)
    for depth in (0, 200, 420):
        print("loop", depth, loop_on_one_depth(depth, 2000))
