"""Purpose: differential coverage for itertools.zip_longest."""

import itertools


def main():
    pairs = list(itertools.zip_longest([1, 2], "abc", fillvalue="X"))
    print("zip_longest", pairs)


if __name__ == "__main__":
    main()
