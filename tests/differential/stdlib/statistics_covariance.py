"""Purpose: differential coverage for statistics.covariance."""

import statistics


def main():
    data1 = [1, 2, 3]
    data2 = [2, 4, 6]
    print("cov", statistics.covariance(data1, data2))


if __name__ == "__main__":
    main()
