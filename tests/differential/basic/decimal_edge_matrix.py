"""Purpose: decimal edge-case matrix for signs, zeros, exponents, and rounding."""

from decimal import (
    Decimal,
    ROUND_DOWN,
    ROUND_HALF_EVEN,
    ROUND_UP,
    localcontext,
)


def main():
    values = [
        Decimal("-0"),
        Decimal("0"),
        Decimal("0E+2"),
        Decimal("0E-7"),
        Decimal("1.2345"),
        Decimal("-1.2345"),
        Decimal("1E+10"),
        Decimal("1E-7"),
    ]
    for item in values:
        print("repr", item, item.as_tuple(), item.normalize())

    pairs = [
        (Decimal("-0"), Decimal("0")),
        (Decimal("1.0"), Decimal("1")),
        (Decimal("-1.0"), Decimal("-1")),
    ]
    for a, b in pairs:
        print("cmp_total", a, b, a.compare_total(b))

    target = Decimal("0.01")
    for mode in (ROUND_DOWN, ROUND_HALF_EVEN, ROUND_UP):
        with localcontext() as ctx:
            ctx.rounding = mode
            print("quant", mode, Decimal("1.235").quantize(target))
            print("quant_neg", mode, Decimal("-1.235").quantize(target))

    with localcontext() as ctx:
        ctx.prec = 6
        print("div", Decimal("1234") / Decimal("7"))
        print("exp", Decimal("1.2").exp())


if __name__ == "__main__":
    main()
