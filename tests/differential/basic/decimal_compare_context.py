"""Purpose: differential coverage for decimal compare and context precision."""

from decimal import Decimal, localcontext


def main():
    a = Decimal("1.2300")
    b = Decimal("1.23")
    print("compare", a.compare(b))
    print("compare_total", a.compare_total(b))

    with localcontext() as ctx:
        ctx.prec = 3
        result = Decimal("1234") / Decimal("7")
        print("prec", result)


if __name__ == "__main__":
    main()
