"""Purpose: differential coverage for decimal rounding modes."""

from decimal import Decimal, ROUND_UP, ROUND_FLOOR, localcontext


def main():
    value = Decimal("-1.234")

    with localcontext() as ctx:
        ctx.rounding = ROUND_UP
        print("up", value.quantize(Decimal("0.01")))

    with localcontext() as ctx:
        ctx.rounding = ROUND_FLOOR
        print("floor", value.quantize(Decimal("0.01")))


if __name__ == "__main__":
    main()
