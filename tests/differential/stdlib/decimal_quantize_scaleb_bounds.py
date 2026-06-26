"""Purpose: differential coverage for quantize / scaleb under Emin/Emax bounds.

Exercises the _fix engine's exponent-bound handling: quantize target-exponent
range checks, the Subnormal signal, and scaleb's |shift| <= 2*(Emax+prec) limit
plus the overflow/clamp behavior driven by the stored context bounds.
"""

from decimal import (
    Decimal,
    Context,
    localcontext,
    Inexact,
    Rounded,
    Subnormal,
    Clamped,
    InvalidOperation,
    ROUND_HALF_EVEN,
)


def main():
    # quantize within a tight context: Subnormal flag fires below Emin.
    ctx = Context(prec=9, Emin=-5, Emax=5, clamp=0, rounding=ROUND_HALF_EVEN)
    with localcontext(ctx) as c:
        c.clear_flags()
        r = Decimal("1.23456789").quantize(Decimal("1e-7"))
        print("quantized", r)
        print("subnormal_flag", c.flags[Subnormal])
        print("inexact_flag", c.flags[Inexact])
        print("rounded_flag", c.flags[Rounded])

    # quantize target exponent out of [Etiny, Emax] -> InvalidOperation.
    with localcontext(Context(prec=9, Emin=-5, Emax=5)):
        try:
            Decimal("1").quantize(Decimal("1e10"))
            print("no_error")
        except InvalidOperation:
            print("quantize_out_of_bounds_raises")

    # scaleb shifts the exponent; result honors the context bounds via _fix.
    with localcontext(Context(prec=9, Emin=-50, Emax=50)) as c2:
        c2.clear_flags()
        print("scaleb_basic", Decimal("1.5").scaleb(Decimal("3")))
        print("scaleb_neg", Decimal("7").scaleb(Decimal("-4")))

    # scaleb shift beyond 2*(Emax+prec) -> InvalidOperation.
    with localcontext(Context(prec=3, Emin=-9, Emax=9)):
        # limit = 2*(9+3) = 24; a shift of 1000 is out of range.
        try:
            Decimal("1").scaleb(Decimal("1000"))
            print("no_error")
        except InvalidOperation:
            print("scaleb_out_of_range_raises")

    # quantize that triggers Clamped via clamp=1 folddown is reflected too.
    with localcontext(Context(prec=6, Emin=-9, Emax=9, clamp=1)) as c3:
        c3.clear_flags()
        q = Decimal("1.0").quantize(Decimal("1e-2"))
        print("clamp_quantize", q)


if __name__ == "__main__":
    main()
