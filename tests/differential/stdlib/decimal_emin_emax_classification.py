"""Purpose: differential coverage for Decimal Emin/Emax-derived classification.

This is the headline P0 regression test. Before the fix, molt's decimal context
stored no Emin/Emax and fabricated emin = 1 - prec (= -27 at the default prec 28),
so Decimal('1e-100').is_normal() returned False and .is_subnormal() returned True
-- a ~6-orders-of-magnitude divergence from CPython, whose default Emin is -999999.
"""

from decimal import Decimal, getcontext


def main():
    ctx = getcontext()
    # Default context bounds must match CPython exactly.
    print("Emin", ctx.Emin)
    print("Emax", ctx.Emax)
    print("clamp", ctx.clamp)
    print("prec", ctx.prec)
    print("Etiny", ctx.Etiny())
    print("Etop", ctx.Etop())

    # 1e-100 has adjusted exponent -100, far above the default Emin (-999999),
    # so it is a perfectly NORMAL number -- NOT subnormal.
    d = Decimal("1e-100")
    print("1e-100 is_normal", d.is_normal())
    print("1e-100 is_subnormal", d.is_subnormal())
    print("1e-100 number_class", d.number_class())

    # A value whose adjusted exponent is below Etop but with adjusted >= Emin.
    print("1e-999998 is_normal", Decimal("1e-999998").is_normal())
    print("1e-999998 is_subnormal", Decimal("1e-999998").is_subnormal())

    # Zero and specials are never normal nor subnormal.
    print("0 is_normal", Decimal("0").is_normal())
    print("0 is_subnormal", Decimal("0").is_subnormal())
    print("0 number_class", Decimal("0").number_class())
    print("-0 number_class", Decimal("-0").number_class())
    print("inf number_class", Decimal("Infinity").number_class())
    print("-inf number_class", Decimal("-Infinity").number_class())
    print("nan number_class", Decimal("NaN").number_class())
    print("snan number_class", Decimal("sNaN").number_class())

    # Sign-sensitive normal classification.
    print("-123.45 number_class", Decimal("-123.45").number_class())
    print("123.45 number_class", Decimal("123.45").number_class())


if __name__ == "__main__":
    main()
