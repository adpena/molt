"""Purpose: differential coverage for statistics.harmonic_mean."""

import statistics


def main():
    data = [1, 2, 4]
    print("hmean", statistics.harmonic_mean(data))

    try:
        statistics.harmonic_mean([0, 1])
    except Exception as exc:
        print("zero", type(exc).__name__)


if __name__ == "__main__":
    main()
