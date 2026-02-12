"""Purpose: differential coverage for statistics.fmean edge cases."""

import statistics


def main():
    print("fmean", statistics.fmean([1, 2, 3]))
    try:
        statistics.fmean([])
    except Exception as exc:
        print("empty", type(exc).__name__)


if __name__ == "__main__":
    main()
