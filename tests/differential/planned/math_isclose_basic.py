"""Purpose: differential coverage for math.isclose basics."""

import math


def main():
    print("close", math.isclose(0.1 + 0.2, 0.3, rel_tol=1e-9))
    print("inf", math.isclose(float("inf"), float("inf")))
    print("nan", math.isclose(float("nan"), float("nan")))


if __name__ == "__main__":
    main()
