"""Purpose: differential coverage for fractions operations."""

from fractions import Fraction


def main():
    frac = Fraction(3, 4)
    print("add", frac + Fraction(1, 4))
    print("mul", frac * 2)
    print("from_float", Fraction(0.5))


if __name__ == "__main__":
    main()
