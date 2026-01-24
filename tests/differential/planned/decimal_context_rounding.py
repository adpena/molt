"""Purpose: differential coverage for decimal context rounding."""

from decimal import Decimal, getcontext, ROUND_HALF_UP


def main():
    ctx = getcontext().copy()
    ctx.prec = 4
    ctx.rounding = ROUND_HALF_UP
    value = ctx.create_decimal("1.23456")
    print("value", value)
    with ctx:
        result = Decimal("1") / Decimal("8")
        print("div", result)


if __name__ == "__main__":
    main()
