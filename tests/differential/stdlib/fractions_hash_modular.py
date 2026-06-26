"""Purpose: differential coverage for Fraction.__hash__ (CPython modular hash).

Regression for the P0 numeric-hash divergence: a whole-number Fraction whose
numerator exceeds i64 previously hashed to 0 (mass collisions), and a non-whole
Fraction previously hashed to the raw f64 bit pattern. Both must now equal
CPython's exact `_hash_algorithm` (mod 2**61-1). Each printed value is compared
against the same program run under CPython 3.12 by the differential harness.
"""

from fractions import Fraction


def main():
    # Whole-number Fraction with a numerator far beyond i64 — used to hash to 0.
    print("hash(Fraction(10**30))", hash(Fraction(10**30)))
    # A whole Fraction must hash equal to its int.
    print("Fraction(10**30) == int", hash(Fraction(10**30)) == hash(10**30))

    # Non-whole Fractions — used to hash to f64 bit patterns.
    print("hash(Fraction(1,3))", hash(Fraction(1, 3)))
    print("hash(Fraction(-7,2))", hash(Fraction(-7, 2)))
    print("hash(Fraction(22,7))", hash(Fraction(22, 7)))
    print("hash(Fraction(-1,3))", hash(Fraction(-1, 3)))

    # Negative whole and zero.
    print("hash(Fraction(-5))", hash(Fraction(-5)))
    print("hash(Fraction(0))", hash(Fraction(0)))

    # Equal Fractions in unreduced form hash identically (reduction invariant).
    print("reduce-invariant", hash(Fraction(2, 6)) == hash(Fraction(1, 3)))

    # A large denominator beyond i64.
    print("hash(Fraction(1, 10**25))", hash(Fraction(1, 10**25)))

    # Dict/set membership across numeric types is the real-world consequence.
    table = {Fraction(1, 2): "half"}
    print("dict[0.5]", table[0.5])
    s = {Fraction(3, 2)}
    print("1.5 in set", 1.5 in s)


if __name__ == "__main__":
    main()
