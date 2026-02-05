"""Purpose: differential coverage for decimal traps and signals."""

from decimal import Decimal, DivisionByZero, Inexact, localcontext


def main():
    with localcontext() as ctx:
        ctx.traps[DivisionByZero] = True
        try:
            Decimal(1) / Decimal(0)
        except Exception as exc:
            print("divzero", type(exc).__name__)

    with localcontext() as ctx:
        ctx.traps[Inexact] = True
        try:
            Decimal("1") / Decimal("3")
        except Exception as exc:
            print("inexact", type(exc).__name__)


if __name__ == "__main__":
    main()
