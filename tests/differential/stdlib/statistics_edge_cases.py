"""Purpose: differential coverage for statistics edge cases."""

import statistics


def main():
    data = [1, 2, 3, 4]
    print("fmean", statistics.fmean(data))
    print("stdev", round(statistics.stdev(data), 6))

    try:
        statistics.mean([])
    except Exception as exc:
        print("mean_empty", type(exc).__name__)

    try:
        statistics.stdev([1])
    except Exception as exc:
        print("stdev_one", type(exc).__name__)


if __name__ == "__main__":
    main()
