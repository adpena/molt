"""Purpose: differential coverage for decimal quantize/rounding modes."""

from decimal import Decimal, ROUND_DOWN, ROUND_HALF_EVEN, localcontext


def main():
    value = Decimal("1.235")

    with localcontext() as ctx:
        ctx.rounding = ROUND_DOWN
        print("down", value.quantize(Decimal("0.01")))

    with localcontext() as ctx:
        ctx.rounding = ROUND_HALF_EVEN
        print("half_even", value.quantize(Decimal("0.01")))


if __name__ == "__main__":
    main()
