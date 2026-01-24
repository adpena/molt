"""Purpose: differential coverage for decimal quantize traps."""

from decimal import Decimal, Inexact, localcontext


def main():
    with localcontext() as ctx:
        ctx.traps[Inexact] = True
        try:
            Decimal("1.234").quantize(Decimal("0.01"))
        except Exception as exc:
            print("trap", type(exc).__name__)


if __name__ == "__main__":
    main()
