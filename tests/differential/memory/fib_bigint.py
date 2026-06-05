# RC drop-insertion regression (design 20): large-index Fibonacci in BigInt.
#
# fib(20000) overflows i64 almost immediately, so `a` and `b` are heap BigInts
# for nearly every iteration. Each `a + b` allocates a new BigInt and the old `a`
# (now shadowed by the previous `b`) is dead and must be freed. Before drop
# insertion every iteration leaked a BigInt; with it, RSS is bounded by the size
# of the two live BigInts (which grow ~linearly in digit count toward fib(20000),
# a few KB), not by the iteration count.
#
# The printed value is reduced mod 1e9+7 so stdout is a small, stable integer
# that is byte-identical to CPython. Run under `safe_run.py --rss-mb 50`.
def fib_mod(n, m):
    a = 0
    b = 1
    i = 0
    while i < n:
        a, b = b, a + b
        i = i + 1
    return a % m


print(fib_mod(20000, 1000000007))
