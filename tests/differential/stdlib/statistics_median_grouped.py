"""Purpose: differential coverage for statistics.median_grouped."""

import statistics


def main():
    data = [1, 2, 2, 3, 4]
    print("median_grouped", statistics.median_grouped(data))


if __name__ == "__main__":
    main()
