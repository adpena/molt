"""Purpose: differential coverage for math core helpers."""

import math


def main() -> None:
    print(math.trunc(3.7))
    print(math.trunc(-3.7))
    print(math.floor(3.2))
    print(math.floor(-3.2))
    print(math.ceil(3.2))
    print(math.ceil(-3.2))
    print(math.fabs(-2.5))
    print(math.copysign(1.0, -0.0))
    print(math.copysign(-1.0, 2.0))
    print(math.fmod(-3.5, 2.0))
    print(math.modf(1.25))
    print(math.modf(-1.25))
    print(math.frexp(8.0))
    print(math.ldexp(0.5, 4))
    print(math.isclose(1.0, 1.0 + 1e-10))
    print(math.isclose(1.0, 1.1))
    print(math.prod([1, 2, 3], start=2))
    print(math.fsum([0.1, 0.2, 0.3]))
    print(math.gcd(12, 18, 9))
    print(math.lcm(3, 4, 5))
    print(math.factorial(5))
    print(math.comb(5, 2))
    print(math.perm(5, 2))
    print(math.degrees(math.pi))
    print(math.radians(180.0))
    print(math.hypot(3.0, 4.0))


if __name__ == "__main__":
    main()
