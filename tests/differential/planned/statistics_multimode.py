"""Purpose: differential coverage for statistics.multimode."""

import statistics


def main():
    data = [1, 2, 2, 3, 3]
    print("multimode", statistics.multimode(data))

    data2 = [1, 2, 3]
    print("unique", statistics.multimode(data2))

    try:
        statistics.multimode([])
    except Exception as exc:
        print("empty", type(exc).__name__)


if __name__ == "__main__":
    main()
