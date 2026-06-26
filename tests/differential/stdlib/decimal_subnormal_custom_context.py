"""Purpose: differential coverage for subnormal classification under a custom Emin.

With a narrowed Emin the same value flips from normal to subnormal, and
number_class reflects it. Exercises the stored-context-bounds path
(getcontext/setcontext/localcontext + Context(...) constructor keyword args).
"""

from decimal import Decimal, Context, localcontext, ROUND_HALF_EVEN


def main():
    # Construct a Context with explicit bounds via the CPython keyword constructor.
    ctx = Context(prec=9, Emin=-95, Emax=96, clamp=0, rounding=ROUND_HALF_EVEN)
    print("ctx Emin", ctx.Emin)
    print("ctx Emax", ctx.Emax)
    print("ctx Etiny", ctx.Etiny())
    print("ctx Etop", ctx.Etop())

    with localcontext(ctx) as c:
        # adjusted(1e-100) = -100 < Emin(-95) -> subnormal.
        d = Decimal("1e-100")
        print("1e-100 is_normal", d.is_normal())
        print("1e-100 is_subnormal", d.is_subnormal())
        print("1e-100 number_class", d.number_class())

        # adjusted(1e-50) = -50 >= Emin -> normal.
        n = Decimal("1e-50")
        print("1e-50 is_normal", n.is_normal())
        print("1e-50 is_subnormal", n.is_subnormal())
        print("1e-50 number_class", n.number_class())

        # Negative subnormal.
        print("-1E-100 number_class", Decimal("-1E-100").number_class())
        _ = c

    # localcontext(**kwargs) override form (CPython 3.11+).
    with localcontext(prec=6, Emin=-10, Emax=10) as c2:
        print("override Emin", c2.Emin)
        print("override Emax", c2.Emax)
        print("override Etiny", c2.Etiny())


if __name__ == "__main__":
    main()
