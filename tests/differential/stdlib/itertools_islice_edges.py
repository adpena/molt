"""Purpose: differential coverage for itertools.islice edge cases."""

import itertools


def main():
    data = range(10)
    print("slice_step", list(itertools.islice(data, 2, 9, 3)))
    print("slice_head", list(itertools.islice(data, 5)))
    try:
        list(itertools.islice(data, -1, 3))
    except Exception as exc:
        print("neg", type(exc).__name__)


if __name__ == "__main__":
    main()
