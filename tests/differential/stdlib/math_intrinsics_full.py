"""Purpose: exercise intrinsic-backed math helpers beyond core predicates."""

import math


def show(label, fn):
    try:
        print(label, fn())
    except Exception as exc:  # pragma: no cover - parity diff helper
        print(label, type(exc).__name__, exc)


def main() -> None:
    print("prod_empty", math.prod([]))
    print("prod_empty_start", math.prod([], start=5))
    print("prod_basic", math.prod([2, 3], start=4))
    print("fsum_basic", math.fsum([0.1, 0.2, 0.3]))
    print("isclose_basic", math.isclose(1.0, 1.0 + 1e-10))
    print("isclose_abs", math.isclose(1.0, 1.0 + 1e-12, abs_tol=1e-9))
    show("isclose_neg_tol", lambda: math.isclose(1.0, 1.0, rel_tol=-1.0))

    print("gcd_none", math.gcd())
    print("gcd_mix", math.gcd(0, -4, 6))
    print("lcm_none", math.lcm())
    print("lcm_zero", math.lcm(0, 5))

    print("factorial_10", math.factorial(10))
    show("factorial_neg", lambda: math.factorial(-1))

    print("comb_basic", math.comb(10, 3))
    print("comb_large", math.comb(50, 6))
    show("comb_neg", lambda: math.comb(5, -1))

    print("perm_basic", math.perm(10, 3))
    print("perm_default", math.perm(6))
    show("perm_neg", lambda: math.perm(5, -2))

    print("degrees_pi", math.degrees(math.pi))
    print("radians_180", math.radians(180.0))
    print("hypot_empty", math.hypot())
    print("hypot_basic", math.hypot(3.0, 4.0))


if __name__ == "__main__":
    main()
