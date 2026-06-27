# Same division-family carrier-store bug class as mod_const_loop_iv.py, for
# floor division. `i // 3` with a bounded loop IV is value-range provable, so
# the result is a raw-i64 carrier; the const divisor forces the boxed fallback
# store, which must go through the carrier-aware store, not the raw one.
def f(n):
    i = 0
    while i < n:
        print(i // 3)
        i = i + 1


f(12)
