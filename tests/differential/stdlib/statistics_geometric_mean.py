"""Purpose: differential coverage for statistics.geometric_mean."""

import statistics


def main():
    data = [1, 4, 9]
    print("gmean", statistics.geometric_mean(data))


if __name__ == "__main__":
    main()
