# MOLT_META: stderr=exact
"""Purpose: differential coverage for decimal basic."""

import decimal


ctx = decimal.getcontext().copy()
ctx.prec = 6
val = ctx.divide(decimal.Decimal("1"), decimal.Decimal("3"))
print(val)

q = decimal.Decimal("1.2345").quantize(decimal.Decimal("0.01"))
print(q)

# Square root keeps an exact result's preferred exponent. Inexact results
# round half-even even when the context requests a different rounding mode.
for precision in (1, 7, 28):
    for rounding in (decimal.ROUND_DOWN, decimal.ROUND_UP):
        with decimal.localcontext() as active:
            active.prec = precision
            active.rounding = rounding
            for text in (
                "1",
                "1.0",
                "1.00",
                "2",
                "2.25",
                "2.2500",
                "1e1000",
                "1e-1000",
                "-0.000",
                "0e1000",
            ):
                active.clear_flags()
                result = decimal.Decimal(text).sqrt()
                print(
                    "sqrt",
                    precision,
                    rounding,
                    text,
                    str(result),
                    active.flags[decimal.Inexact],
                    active.flags[decimal.Rounded],
                )

with decimal.localcontext() as active:
    active.prec = 3
    active.Emin = -2
    active.Emax = 2
    active.traps[decimal.InvalidOperation] = False
    active.traps[decimal.Overflow] = False
    for text in ("1e-9", "1e-8", "1e6", "-1", "-Infinity", "NaN", "sNaN"):
        active.clear_flags()
        result = active.sqrt(decimal.Decimal(text))
        print(
            "sqrt bounds",
            text,
            str(result),
            tuple(
                active.flags[signal]
                for signal in (
                    decimal.InvalidOperation,
                    decimal.Overflow,
                    decimal.Inexact,
                    decimal.Rounded,
                    decimal.Subnormal,
                    decimal.Underflow,
                    decimal.Clamped,
                )
            ),
        )

with decimal.localcontext() as active:
    active.traps[decimal.Inexact] = True
    print("sqrt exact trap", decimal.Decimal("1.00").sqrt())
    try:
        decimal.Decimal(2).sqrt()
    except decimal.Inexact:
        print("sqrt inexact trap", active.flags[decimal.Rounded])

# Legal precision must not become an arbitrary power-of-ten admission limit.
# Print bounded output while the CPython oracle checks both exact admission
# and the full-precision inexact path, rather than merely a short fast path.
with decimal.localcontext() as active:
    active.prec = 100001
    for text in ("1", "2"):
        active.clear_flags()
        result = str(decimal.Decimal(text).sqrt())
        print(
            "sqrt large precision",
            text,
            len(result),
            result[:20],
            result[-20:],
            active.flags[decimal.Inexact],
            active.flags[decimal.Rounded],
        )
