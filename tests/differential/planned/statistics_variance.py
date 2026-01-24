"""Purpose: differential coverage for statistics variance/stdev."""

import statistics


def main():
    data = [2, 4, 4, 4, 5, 5, 7, 9]
    print("variance", statistics.variance(data))
    print("pstdev", round(statistics.pstdev(data), 6))


if __name__ == "__main__":
    main()
