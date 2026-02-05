"""Purpose: differential coverage for math.fma and math.remainder."""

import math


def show(label, fn):
    try:
        print(label, fn())
    except Exception as exc:  # pragma: no cover - parity diff helper
        print(label, type(exc).__name__)


def main() -> None:
    if hasattr(math, "fma"):
        print("fma_basic", math.fma(1.0, 2.0, 3.0))
        print("fma_inf", math.fma(1.0, 2.0, float("inf")))
        print("fma_nan", math.fma(float("nan"), 2.0, 3.0))
    else:
        print("fma_missing")

    print("remainder_basic", math.remainder(1.0, 2.0))
    print("remainder_neg", math.remainder(-1.0, 2.0))
    print("remainder_neg_div", math.remainder(1.0, -2.0))
    print("remainder_inf_div", math.remainder(1.0, float("inf")))
    show("remainder_zero", lambda: math.remainder(1.0, 0.0))
    show("remainder_inf", lambda: math.remainder(float("inf"), 2.0))
    print("remainder_nan", math.remainder(float("nan"), 2.0))


if __name__ == "__main__":
    main()
