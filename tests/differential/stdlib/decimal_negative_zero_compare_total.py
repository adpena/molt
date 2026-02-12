"""Purpose: decimal negative-zero formatting and compare_total parity."""

from decimal import Decimal


def main():
    neg_zero = Decimal("-0")
    pos_zero = Decimal("0")
    print("str", neg_zero)
    print("tuple", neg_zero.as_tuple())
    print("cmp_total", neg_zero.compare_total(pos_zero))


if __name__ == "__main__":
    main()
