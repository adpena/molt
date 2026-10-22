"""Purpose: recursion deeper than the native stack ends every native code owner the same way.

CPython 3.12 keeps Python frames off the C stack, so this recursion reaches the
raised limit and raises RecursionError. A Molt native program recurses on the
native stack, which runs out first; with no handler installed, as in a C
program, the process ends with SIGSEGV. The development JIT runner must not
add Rust's stack-overflow handler, which aborts with a message instead (HF-371).
"""
# MOLT_META: expect_fail=molt expect_fail_reason=native_stack_exhaustion

import sys

sys.setrecursionlimit(1_000_000)


def descend(depth):
    return descend(depth + 1) + 1


try:
    descend(0)
except RecursionError:
    print("RecursionError")
