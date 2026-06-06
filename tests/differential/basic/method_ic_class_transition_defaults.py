"""Purpose: fused method-call IC with DEFAULT-binding methods must invalidate
correctly on a class transition at a single call site (megamorphic-ish A,A,A,B).

The fix that routes class/version-valid cached entries through the cached-bind
path must still treat a class_bits/version MISMATCH as a genuine miss -> resolve
+ re-insert.  This drives one call site with a sequence of receivers of
different classes, each whose method requires full binding (positional default),
so a stale cached `func_bits` would return the WRONG class's result.

The `dispatch` helper holds the single polymorphic call site `obj.label(n)`.
"""


class A:
    def label(self, n, suffix="-A"):
        return "A" + str(n) + suffix


class B:
    def label(self, n, suffix="-B"):
        return "B" + str(n) + suffix


class C(A):
    # Inherits A.label but overrides the default -> distinct shape/version.
    def label(self, n, suffix="-C"):
        return "C" + str(n) + suffix


def dispatch(obj, n):
    # Single call site, exercised with A, A, A, then B, then C, ... so the IC
    # for this site sees a class transition and must re-resolve.
    return obj.label(n)


def dispatch_kw(obj, n, s):
    return obj.label(n, suffix=s)


def main() -> None:
    a = A()
    b = B()
    c = C()

    # Warm with A (A,A,A) then transition to B, then C, then back to A, in a
    # loop so each transition crosses a warm IC entry repeatedly.
    out = []
    seq = [a, a, a, b, c, a, b, b, c, c, a]
    for r in range(100):
        for obj in seq:
            out.append(dispatch(obj, r))
    # Print a stable digest: counts per class + a sample of the first cycle.
    print("count_A:", sum(1 for x in out if x.startswith("A")))
    print("count_B:", sum(1 for x in out if x.startswith("B")))
    print("count_C:", sum(1 for x in out if x.startswith("C")))
    print("first_cycle:", out[: len(seq)])

    # Explicit-keyword default override across the transition too.
    print("kw_A:", dispatch_kw(a, 1, "-X"))
    print("kw_B:", dispatch_kw(b, 2, "-Y"))
    print("kw_C:", dispatch_kw(c, 3, "-Z"))

    # Defaults after transition (no keyword) — proves the cached entry per class
    # is the right one.
    print("def_A:", dispatch(a, 9))
    print("def_B:", dispatch(b, 9))
    print("def_C:", dispatch(c, 9))


if __name__ == "__main__":
    main()
