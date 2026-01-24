"""Purpose: differential coverage for decimal exp/quantize signals."""

from decimal import Decimal, Inexact, localcontext


def main():
    with localcontext() as ctx:
        ctx.clear_flags()
        result = Decimal("1.2345").quantize(Decimal("0.01"))
        print("quantized", result)
        print("inexact", ctx.flags[Inexact])

    with localcontext() as ctx:
        ctx.prec = 6
        result = Decimal("1.2").exp()
        print("exp", result)


if __name__ == "__main__":
    main()
