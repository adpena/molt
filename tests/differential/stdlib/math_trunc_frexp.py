"""Purpose: differential coverage for math.trunc/frexp."""

import math


def main():
    print("trunc", math.trunc(3.9))
    print("frexp", math.frexp(8.0))


if __name__ == "__main__":
    main()
