"""Purpose: differential coverage for statistics median_low/high."""

import statistics


def main():
    data = [1, 2, 3, 4]
    print("median_low", statistics.median_low(data))
    print("median_high", statistics.median_high(data))


if __name__ == "__main__":
    main()
