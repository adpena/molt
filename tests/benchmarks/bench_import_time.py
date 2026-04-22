"""Measures repeated module import overhead."""

import importlib


def main() -> None:
    total = 0
    for i in range(10000):
        mod = importlib.import_module("json")
        total += len(dir(mod))
    print(total)


if __name__ == "__main__":
    main()
