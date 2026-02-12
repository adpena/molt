"""Purpose: differential coverage for math trig/hyperbolic intrinsics."""

import math


def show(label, fn):
    try:
        print(label, fn())
    except Exception as exc:  # pragma: no cover - parity diff helper
        print(label, type(exc).__name__)


def main() -> None:
    print("tan_zero", math.tan(0.0))
    print("asin_zero", math.asin(0.0))
    print("acos_one", math.acos(1.0))
    print("atan_one", math.atan(1.0))
    print("atan2_basic", math.atan2(0.0, -1.0))
    print("sinh_zero", math.sinh(0.0))
    print("cosh_zero", math.cosh(0.0))
    print("tanh_zero", math.tanh(0.0))
    print("asinh_zero", math.asinh(0.0))
    print("acosh_one", math.acosh(1.0))
    print("atanh_zero", math.atanh(0.0))

    show("asin_oob", lambda: math.asin(2.0))
    show("acos_oob", lambda: math.acos(2.0))
    show("acosh_oob", lambda: math.acosh(0.5))
    show("atanh_oob", lambda: math.atanh(1.0))
    show("tan_inf", lambda: math.tan(float("inf")))


if __name__ == "__main__":
    main()
