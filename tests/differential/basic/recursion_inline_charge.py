"""Purpose: the inline recursion charge of compiled calls (call_anatomy.md stage 2).

Each call kind counts one frame against the runtime's one limit, a new limit
reaches the next check without rebinding, and a call that exits by raising
releases its frame, so a second probe reaches the same depth.
"""

import sys


def probe(n):
    try:
        return probe(n + 1)
    except RecursionError:
        return n


class Walker:
    def probe(self, n):
        try:
            return self.probe(n + 1)
        except RecursionError:
            return n


def closure_probe():
    def inner(n):
        try:
            return inner(n + 1)
        except RecursionError:
            return n

    return inner


old = sys.getrecursionlimit()
try:
    for name, run in (
        ("function", lambda: probe(0)),
        ("method", lambda: Walker().probe(0)),
        ("closure", lambda: closure_probe()(0)),
    ):
        sys.setrecursionlimit(300)
        low = run()
        again = run()
        sys.setrecursionlimit(500)
        high = run()
        print(name, low == again, high - low)
finally:
    sys.setrecursionlimit(old)
