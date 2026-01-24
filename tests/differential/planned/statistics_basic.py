"""Purpose: differential coverage for statistics basics."""

import statistics


def main():
    data = [1, 2, 2, 3]
    print("mean", statistics.mean(data))
    print("median", statistics.median(data))
    print("mode", statistics.mode(data))
    print("pvariance", statistics.pvariance(data))


if __name__ == "__main__":
    main()
