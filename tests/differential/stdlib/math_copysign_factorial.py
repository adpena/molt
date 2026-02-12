"""Purpose: differential coverage for math.copysign/factorial."""

import math


def main():
    print("copysign", math.copysign(2.0, -1.0))
    print("factorial", math.factorial(5))


if __name__ == "__main__":
    main()
