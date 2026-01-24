"""Purpose: differential coverage for statistics.correlation."""

import statistics


def main():
    data1 = [1, 2, 3, 4]
    data2 = [2, 4, 6, 8]
    print("corr", statistics.correlation(data1, data2))


if __name__ == "__main__":
    main()
