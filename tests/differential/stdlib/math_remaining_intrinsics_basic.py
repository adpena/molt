"""Purpose: differential coverage for remaining math intrinsics."""

import math


def show(label, fn):
    try:
        print(label, fn())
    except Exception as exc:  # pragma: no cover - parity diff helper
        print(label, type(exc).__name__)


def main() -> None:
    print("log10_1", math.log10(1.0))
    show("log10_zero", lambda: math.log10(0.0))
    show("log10_neg", lambda: math.log10(-1.0))

    print("log1p_half", math.log1p(-0.5))
    show("log1p_neg1", lambda: math.log1p(-1.0))
    print("log1p_inf", math.log1p(float("inf")))

    print("expm1_1", math.expm1(1.0))
    print("expm1_neg_inf", math.expm1(float("-inf")))
    show("expm1_big", lambda: math.expm1(1000.0))

    print("gamma_1", math.gamma(1.0))
    print("gamma_2_5", math.gamma(2.5))
    show("gamma_zero", lambda: math.gamma(0.0))
    show("gamma_neg_inf", lambda: math.gamma(float("-inf")))

    print("erf_0", math.erf(0.0))
    print("erfc_0", math.erfc(0.0))

    print("dist_basic", math.dist([0, 0], [3, 4]))
    show("dist_len", lambda: math.dist([0], [0, 1]))
    show("dist_noniter", lambda: math.dist(1, 2))

    print("isqrt_10", math.isqrt(10))
    show("isqrt_neg", lambda: math.isqrt(-1))

    print("nextafter_up", math.nextafter(0.0, 1.0))
    print("nextafter_down", math.nextafter(0.0, -1.0))
    print("nextafter_inf", math.nextafter(float("inf"), 0.0))

    print("ulp_0", math.ulp(0.0))
    print("ulp_1", math.ulp(1.0))
    print("ulp_inf", math.ulp(float("inf")))
    print("ulp_nan", math.ulp(float("nan")))


if __name__ == "__main__":
    main()
