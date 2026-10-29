"""Purpose: trapped decimal signals raise decimal's own classes and stay out of builtins."""

import builtins
import decimal
from decimal import Context, Decimal


def probe(label, func, expected):
    try:
        func()
    except expected as exc:
        print(
            label,
            expected.__name__,
            type(exc).__name__,
            type(exc).__module__,
            isinstance(exc, decimal.DecimalException),
            isinstance(exc, ArithmeticError),
        )
    except Exception as exc:  # noqa: BLE001 - the observation is the escape
        print(label, "escaped", type(exc).__name__, type(exc).__module__)
    else:
        print(label, "no error")


def main():
    names = (
        "DecimalException",
        "InvalidOperation",
        "DivisionByZero",
        "Inexact",
        "Overflow",
        "Underflow",
        "Subnormal",
        "Rounded",
        "Clamped",
        "ConversionSyntax",
        "DivisionImpossible",
        "DivisionUndefined",
        "InvalidContext",
        "FloatOperation",
    )
    for name in names:
        print("builtins has", name, hasattr(builtins, name))
    try:
        InvalidOperation  # noqa: B018, F821 - must be undefined
    except NameError:
        print("bare InvalidOperation is NameError")
    probe("divide by zero", lambda: Decimal(1) / Decimal(0), decimal.DivisionByZero)
    probe("conversion", lambda: Decimal("not a number"), decimal.InvalidOperation)
    probe("sqrt negative", lambda: Decimal(-1).sqrt(), decimal.InvalidOperation)
    overflow = Context(Emax=1, traps=[decimal.Overflow])
    probe("overflow", lambda: overflow.multiply(Decimal(10), Decimal(10)), decimal.Overflow)
    inexact = Context(prec=2, traps=[decimal.Inexact])
    probe("inexact", lambda: inexact.divide(Decimal(1), Decimal(3)), decimal.Inexact)
    floats = Context(traps=[decimal.FloatOperation])
    probe("float operation", lambda: floats.create_decimal(1.5), decimal.FloatOperation)


if __name__ == "__main__":
    main()
