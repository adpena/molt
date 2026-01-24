"""Purpose: differential coverage for statistics.linear_regression."""

import statistics


def main():
    slope, intercept = statistics.linear_regression([1, 2, 3], [2, 4, 6])
    print("slope", slope)
    print("intercept", intercept)


if __name__ == "__main__":
    main()
