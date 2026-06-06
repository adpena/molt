"""Purpose: fused method-call IC must apply defaults / kw-only / *args / **kwargs
through the CACHED-BIND path in a hot loop (no re-resolve per call).

Regression for the asyncio P0 follow-on: the fused method IC originally gated the
allocation-free `call_direct` fast path INSIDE the cache-validity check, so a
method requiring full binding (positional default, kw-only, *args, **kwargs)
either (a) raised a spurious "call arity mismatch" or (b) fell into a per-call
MRO re-resolve.  The fix routes class/version-valid cached entries with a
non-direct shape to the binder while REUSING the cached resolution.

This program drives each shape through a hot loop so the IC is warm and the
cached-bind path (not the first-call resolve) is exercised, and it checks the
returned VALUES so a wrong-default or arg-shift bug is also caught.

PERF CONTRACT: the positional-default shape `obj.m(i)` over `def m(self, x,
bump=1)` (mirrored by `Accum.add` below) must, when compiled native, run at or
below the no-IC baseline AND faster than CPython 3.14 on the same program. The
microbench lives in tests/benchmarks/bench_method_default_binding.py.
"""


class Accum:
    # Positional default: the exact reviewer microbench shape.
    def add(self, x, bump=1):
        return x + bump

    # Keyword-only with default.
    def scale(self, x, *, factor=2):
        return x * factor

    # Keyword-only without default (must be supplied by keyword).
    def shift(self, x, *, by):
        return x + by

    # *args.
    def total(self, *args):
        return sum(args)

    # **kwargs.
    def bag(self, **kwargs):
        return len(kwargs)

    # No default, exact arity: the allocation-free direct fast path.
    def double(self, x):
        return x * 2


def main() -> None:
    a = Accum()

    # Hot loop: warm the per-site IC and exercise the cached-bind path many
    # times.  Accumulate so a single wrong call is observable in the total.
    s_add_default = 0
    s_add_explicit = 0
    s_scale_default = 0
    s_scale_explicit = 0
    s_shift = 0
    s_total = 0
    s_bag = 0
    s_double = 0
    for i in range(2000):
        s_add_default += a.add(i)  # bump defaulted -> i + 1
        s_add_explicit += a.add(i, bump=10)  # bump=10  -> i + 10
        s_add_explicit += a.add(i, 100)  # positional bump=100 -> i + 100
        s_scale_default += a.scale(i)  # factor defaulted -> i * 2
        s_scale_explicit += a.scale(i, factor=3)  # i * 3
        s_shift += a.shift(i, by=5)  # i + 5
        s_total += a.total(i, i + 1, i + 2)  # 3i + 3
        s_bag += a.bag(p=1, q=2, r=3)  # 3
        s_double += a.double(i)  # i * 2

    print("add_default:", s_add_default)
    print("add_explicit:", s_add_explicit)
    print("scale_default:", s_scale_default)
    print("scale_explicit:", s_scale_explicit)
    print("shift:", s_shift)
    print("total:", s_total)
    print("bag:", s_bag)
    print("double:", s_double)

    # Spot-check exact single-call values (catches an off-by-one / arg-shift
    # that might still sum to a coincidentally-equal total).
    print("spot add():", a.add(7))
    print("spot add(,bump=3):", a.add(7, bump=3))
    print("spot add(,2):", a.add(7, 2))
    print("spot scale():", a.scale(7))
    print("spot scale(factor=4):", a.scale(7, factor=4))
    print("spot shift(by=9):", a.shift(7, by=9))
    print("spot total(1,2,3,4):", a.total(1, 2, 3, 4))
    print("spot bag(x=1,y=2):", a.bag(x=1, y=2))
    print("spot double():", a.double(7))


if __name__ == "__main__":
    main()
