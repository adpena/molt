"""Purpose: decimal constructor should ignore active context precision."""

from decimal import Decimal, localcontext


def main():
    with localcontext() as ctx:
        ctx.prec = 3
        print("ctor", Decimal("1.2345"))
        print("bool", Decimal(True))


if __name__ == "__main__":
    main()
