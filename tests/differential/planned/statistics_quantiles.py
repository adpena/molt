"""Purpose: differential coverage for statistics.quantiles."""

import statistics


def main():
    data = [1, 2, 3, 4, 5, 6, 7, 8]
    print("quantiles", statistics.quantiles(data, n=4))


if __name__ == "__main__":
    main()
