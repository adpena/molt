# Companion to mod_const_loop_iv.py: the `k = 7; i % k` variant. Here the
# divisor is a named variable, exercising the raw-primary mod lane when both
# operands are raw-i64 carriers, and the boxed fallback store otherwise. The
# result is value-range proven `[0, 7)` -> raw-i64 carrier, so the boxed-vs-raw
# store contract must hold here too.
def f(n):
    k = 7
    i = 0
    while i < n:
        print(i % k)
        i = i + 1


f(10)
