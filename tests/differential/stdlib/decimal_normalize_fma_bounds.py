"""Purpose: differential coverage for normalize and fma routed through _fix.

normalize must round to the context first, then strip trailing zeros without
raising the exponent above exp_max; fma applies a single final rounding under
the context bounds. Both previously ignored Emin/Emax entirely.
"""

from decimal import Decimal, Context, localcontext, ROUND_HALF_EVEN


def main():
    # normalize strips trailing zeros and maps any zero to 0E0.
    print("normalize_1.200", Decimal("1.200").normalize())
    print("normalize_0.00", Decimal("0.00").normalize())
    print("normalize_-0.0", Decimal("-0.0").normalize())
    print("normalize_120000", Decimal("120000").normalize())
    print("normalize_10000.0", Decimal("10000.0").normalize())

    # Under a small precision, normalize first rounds to prec then strips zeros.
    with localcontext(Context(prec=4, Emin=-20, Emax=20, rounding=ROUND_HALF_EVEN)):
        print("normalize_prec", Decimal("1.23456").normalize())
        print("normalize_big", Decimal("123450000").normalize())

    # fma: exact product, single final rounding under the context.
    print("fma_basic", Decimal("3").fma(Decimal("4"), Decimal("5")))
    print("fma_frac", Decimal("0.1").fma(Decimal("0.1"), Decimal("0.01")))
    with localcontext(Context(prec=5, Emin=-99, Emax=99)):
        print("fma_round", Decimal("1.2345").fma(Decimal("6.789"), Decimal("0.0001")))


if __name__ == "__main__":
    main()
