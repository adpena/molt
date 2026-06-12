"""Purpose: compile-time devirtualization of a defaults-bearing method call must
remain SEMANTICALLY correct when ``Cls.method.__defaults__`` is reassigned at
runtime.

CPython binds ``__defaults__`` at CALL time, so reassigning the tuple changes
which value a later short call uses.  molt devirtualizes ``obj.m(i)`` to a direct
compiled CALL and bakes the literal default for speed; the
``__defaults__``-mutation deopt guard (a monotonic version stamp on the function
object) catches any reassignment and routes the call to the live-reading dynamic
path.  This pins the observable.

Covers:
  * baked-literal fast path BEFORE any mutation (warm-up calls);
  * reassignment of a POSITIONAL default mid-program -> subsequent short calls
    must use the NEW default;
  * reassignment of ``__kwdefaults__`` for a kw-only default;
  * mid-loop reassignment after warm-up (the guard must fire per-iteration);
  * reassigning back to a fresh tuple of the same value (version still bumps,
    value still correct);
  * a method whose default is engaged on some calls and supplied on others.

Byte-identical vs CPython 3.12 / 3.13 / 3.14.
"""


class Obj:
    def m(self, x, bump=1):
        return x + bump


class KW:
    def k(self, x, *, step=10):
        return x * step


def main() -> None:
    out = []

    o = Obj()
    # Warm-up: baked-literal fast path, default engaged.
    for i in range(5):
        out.append(o.m(i))  # x + 1
    # Supply the default explicitly (no padding) interleaved.
    out.append(o.m(100, 7))  # 107

    # Reassign the positional default.  CPython: later short calls use 50.
    Obj.m.__defaults__ = (50,)
    for i in range(5):
        out.append(o.m(i))  # x + 50
    out.append(o.m(100, 7))  # still 107 (explicit arg wins)

    # Reassign again to a freshly-built tuple holding the same value: version
    # bumps but the observed default is unchanged.
    Obj.m.__defaults__ = (50,)
    out.append(o.m(0))  # 50

    # Reassign to a different value once more.
    Obj.m.__defaults__ = (-3,)
    out.append(o.m(10))  # 7

    # kw-only default via __kwdefaults__ mutation.
    kw = KW()
    out.append(kw.k(2))  # 2 * 10 = 20
    KW.k.__kwdefaults__ = {"step": 3}
    out.append(kw.k(2))  # 2 * 3 = 6
    out.append(kw.k(2, step=100))  # explicit wins -> 200

    print(out)
    print("final Obj.m.__defaults__:", Obj.m.__defaults__)
    print("final KW.k.__kwdefaults__:", KW.k.__kwdefaults__)

    # Mid-loop reassignment after warm-up: a hot call site whose default
    # changes once partway through.  Every post-mutation call must reflect it.
    o2 = Obj()
    Obj.m.__defaults__ = (1,)  # reset to original
    acc = []
    for i in range(10):
        if i == 5:
            Obj.m.__defaults__ = (1000,)
        acc.append(o2.m(i))
    print("mid_loop:", acc)


if __name__ == "__main__":
    main()
