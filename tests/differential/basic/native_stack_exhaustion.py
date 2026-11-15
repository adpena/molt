"""Purpose: recursion far past the OS default stack reaches the recursion limit.

CPython 3.12 keeps Python frames off the C stack, so this recursion reaches the
raised limit and raises RecursionError. Molt recurses on the native stack, and
runs compiled frames on a reserved program stack (molt-runtime
state/program_stack.rs), so it reaches the same limit on both the AOT and the
development JIT tiers. Before that stack (HF-371) the process ended with SIGSEGV.
"""
# MOLT_META: backends=jit,native platforms=linux,macos

import sys

sys.setrecursionlimit(1_000_000)


def descend(depth):
    return descend(depth + 1) + 1


print("descending past the native stack", flush=True)
try:
    descend(0)
except RecursionError:
    print("RecursionError")
