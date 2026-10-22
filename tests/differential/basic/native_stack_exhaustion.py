"""Purpose: recursion deeper than the native stack ends every native code owner the same way.

CPython 3.12 keeps Python frames off the C stack, so this recursion reaches the
raised limit and raises RecursionError. A Molt native program recurses on the
native stack, which runs out first; with no handler installed, as in a C
program, the process ends with SIGSEGV. The development JIT runner must not
add Rust's stack-overflow handler, which aborts with a message instead (HF-371).

The program prints a marker line before the recursion. The pinned Molt
outcome, for both tiers: Molt prints the marker, then ends with SIGSEGV (exit
-11). A crash before the recursion, any other ending, or two tiers that end
differently, fails the case.
"""
# MOLT_META: backends=jit,native platforms=linux,macos expect_fail=molt expect_fail_reason=native_stack_exhaustion expect_molt_returncode=-11 expect_molt_stdout_sha256=1f8c715255b8ade44559df682c53bfe661e6007c61a37e7f83eb8cbda5000c32

import sys

sys.setrecursionlimit(1_000_000)


def descend(depth):
    return descend(depth + 1) + 1


# Flushed now: a process that a signal ends loses buffered output.
print("descending past the native stack", flush=True)
try:
    descend(0)
except RecursionError:
    print("RecursionError")
