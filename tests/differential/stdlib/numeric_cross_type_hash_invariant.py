"""Purpose: cross-type numeric-hash invariant (int/float/Fraction/Decimal).

CPython guarantees that numerically-equal numbers hash equal across types so
they collide in the same dict/set bucket. This pins the invariant that the
single shared `py_numeric_hash` authority exists to uphold. Compared against
CPython 3.12 by the differential harness.
"""

from decimal import Decimal
from fractions import Fraction


def main():
    # hash(1) == hash(1.0) == hash(Fraction(1)) == hash(Decimal(1))
    h1 = hash(1)
    print("ints-equal", h1 == hash(1.0) == hash(Fraction(1)) == hash(Decimal(1)))
    print("value hash(1)", h1)

    # hash(Fraction(3, 2)) == hash(1.5) == hash(Decimal('1.5'))
    print(
        "three-halves",
        hash(Fraction(3, 2)) == hash(1.5) == hash(Decimal("1.5")),
    )
    print("value hash(1.5)", hash(1.5))

    # A value float CANNOT represent exactly: 1/10. Fraction(1,10) and
    # Decimal('0.1') must still agree (they share the modular authority).
    print(
        "one-tenth",
        hash(Fraction(1, 10)) == hash(Decimal("0.1")),
    )
    print("value hash(Fraction(1,10))", hash(Fraction(1, 10)))

    # A big integer value across int / Fraction / Decimal.
    big = 10**30
    print(
        "big-cross",
        hash(big) == hash(Fraction(big)) == hash(Decimal(big)),
    )
    print("value hash(10**30)", hash(big))

    # Cross-type dict membership: all four keys collapse to one entry.
    d = {}
    d[1] = "int"
    d[1.0] = "float"
    d[Fraction(1)] = "fraction"
    d[Decimal(1)] = "decimal"
    print("collapsed-len", len(d), "final", d[1])

    # Negative cross-type.
    print(
        "neg-cross",
        hash(-7) == hash(-7.0) == hash(Fraction(-7)) == hash(Decimal(-7)),
    )


if __name__ == "__main__":
    main()
