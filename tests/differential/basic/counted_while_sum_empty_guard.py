"""Purpose: counted-while sum fast-path must not miscompile the empty-loop case.

When the loop index start constant is >= the bound the loop runs zero times and
the accumulator is unchanged; the arithmetic-series closed-form fold previously
assumed >=1 iteration and emitted a silently-wrong sum (start=10,bound=5 -> -35
instead of 0). The fast path is function-scope only, so cases live inside defs.
Version-stable across CPython 3.12/3.13/3.14.
"""


def sum_binop(start, bound):
    i = start
    s = 0
    while i < bound:
        s = s + i
        i = i + 1
    return s, i


def sum_augassign(start, bound):
    i = start
    s = 0
    while i < bound:
        s += i
        i = i + 1
    return s, i


def sum_acc_nonzero(start, bound):
    i = start
    s = 100
    while i < bound:
        s = s + i
        i = i + 1
    return s, i


print(sum_binop(10, 5))        # (0, 10)
print(sum_augassign(10, 5))    # (0, 10)
print(sum_acc_nonzero(10, 5))  # (100, 10)
print(sum_binop(5, 5))         # (0, 5)
print(sum_augassign(5, 5))     # (0, 5)
print(sum_binop(4, 5))         # (4, 5)
print(sum_augassign(4, 5))     # (4, 5)
print(sum_acc_nonzero(4, 5))   # (104, 5)
print(sum_binop(0, 5))         # (10, 5)
print(sum_binop(3, 10))        # (42, 10)
print(sum_acc_nonzero(3, 10))  # (142, 10)
print(sum_binop(7, 3))         # (0, 7)
print(sum_binop(0, 0))         # (0, 0)
