"""Purpose: differential coverage for Decimal.__hash__ (CPython modular hash).

The previous shim computed `hash(float(self))`, which loses precision for any
Decimal a float cannot represent (e.g. Decimal(10**30), Decimal('0.1')) and so
diverged from CPython's exact `int(coeff) * pow(10, exp, M) % M`. It also raised
TypeError for a quiet NaN (CPython falls back to identity). Compared against
CPython 3.12 by the differential harness.
"""

from decimal import Decimal


def main():
    # Large integral Decimal beyond float's exact range.
    print("hash(Decimal(10**30))", hash(Decimal(10**30)))
    print("Decimal(10**30) == int", hash(Decimal(10**30)) == hash(10**30))

    # Fractional Decimals (negative exponent path: modular inverse of 10**k).
    print("hash(Decimal('0.1'))", hash(Decimal("0.1")))
    print("hash(Decimal('1.5'))", hash(Decimal("1.5")))
    print("hash(Decimal('-2.5'))", hash(Decimal("-2.5")))
    print("hash(Decimal('3.14159'))", hash(Decimal("3.14159")))

    # Positive-exponent path (e.g. 1E+5 == 100000).
    print("hash(Decimal('1E+5'))", hash(Decimal("1E+5")))
    print("Decimal('1E+5') == int", hash(Decimal("1E+5")) == hash(100000))
    # Huge exponents must stay modular; the runtime must not materialize
    # 10**abs(exp) to compute a hash.
    print("hash(Decimal('1e999999'))", hash(Decimal("1e999999")))
    print("hash(Decimal('1e-999999'))", hash(Decimal("1e-999999")))
    print("hash(Decimal('-1e999999'))", hash(Decimal("-1e999999")))
    print("hash(Decimal('-1e-999999'))", hash(Decimal("-1e-999999")))

    # Trailing-zero forms hash equal to the canonical value.
    print("trailing-zero", hash(Decimal("1.50")) == hash(Decimal("1.5")))

    # Zero and negative zero.
    print("hash(Decimal('0'))", hash(Decimal("0")))
    print("hash(Decimal('-0'))", hash(Decimal("-0")))

    # Infinity hashes to +/- _PyHASH_INF.
    print("hash(Decimal('Infinity'))", hash(Decimal("Infinity")))
    print("hash(Decimal('-Infinity'))", hash(Decimal("-Infinity")))

    # Quiet NaN: identity-based, equal to object.__hash__ (no TypeError).
    qnan = Decimal("NaN")
    print("qNaN-identity", hash(qnan) == object.__hash__(qnan))

    # Signaling NaN: TypeError.
    try:
        hash(Decimal("sNaN"))
        print("sNaN-raised", False)
    except TypeError as exc:
        print("sNaN-raised", True, str(exc))


if __name__ == "__main__":
    main()
