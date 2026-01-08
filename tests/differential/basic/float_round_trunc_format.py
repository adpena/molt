import math


def main() -> None:
    print(round(0.5))
    print(round(1.5))
    print(round(-0.5))
    print(round(2.5))
    print(round(1.25, 1))
    print(round(1.35, 1))
    print(round(2.675, 2))
    try:
        print(round(float("nan")))
    except ValueError:
        print("ValueError")
    try:
        print(round(float("inf")))
    except OverflowError:
        print("OverflowError")
    try:
        print(round(float("-inf")))
    except OverflowError:
        print("OverflowError")
    print(round(float("nan"), 2))
    print(round(float("inf"), 2))
    print(round(float("-inf"), 2))
    print("{:.2f}".format(float("nan")))
    print("{:.2f}".format(float("inf")))
    print("{:.2f}".format(float("-inf")))
    try:
        math.trunc(float("nan"))
    except ValueError:
        print("ValueError")
    try:
        math.trunc(float("inf"))
    except OverflowError:
        print("OverflowError")
    print(math.trunc(-1.7))
    print(math.trunc(3.9))


if __name__ == "__main__":
    main()
