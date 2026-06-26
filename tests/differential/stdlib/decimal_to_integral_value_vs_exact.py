"""Purpose: differential coverage for to_integral_value vs to_integral_exact flags.

CPython contract: to_integral_value rounds to an integer WITHOUT raising Inexact
or Rounded; to_integral_exact does the same rounding but DOES raise Inexact (when
the value changed) and Rounded. Before the fix both shared a body that signalled
Inexact|Rounded, so to_integral_value wrongly flagged (and could trap).
"""

from decimal import Decimal, getcontext, Inexact, Rounded


def main():
    ctx = getcontext()

    ctx.clear_flags()
    v = Decimal("123.456").to_integral_value()
    print("value_result", v)
    print("value_inexact", ctx.flags[Inexact])
    print("value_rounded", ctx.flags[Rounded])

    ctx.clear_flags()
    e = Decimal("123.456").to_integral_exact()
    print("exact_result", e)
    print("exact_inexact", ctx.flags[Inexact])
    print("exact_rounded", ctx.flags[Rounded])

    # Exact integral input: no flags from either, and value is unchanged.
    ctx.clear_flags()
    print("exact_on_int", Decimal("42").to_integral_exact())
    print("exact_on_int_inexact", ctx.flags[Inexact])
    print("exact_on_int_rounded", ctx.flags[Rounded])

    # Already-integral with positive exponent stays unchanged.
    print("value_1e2", Decimal("1E+2").to_integral_value())
    print("exact_1e2", Decimal("1E+2").to_integral_exact())

    # Zero short-circuits to 0E0 with no signals.
    ctx.clear_flags()
    print("exact_zero", Decimal("0.000").to_integral_exact())
    print("exact_zero_rounded", ctx.flags[Rounded])


if __name__ == "__main__":
    main()
