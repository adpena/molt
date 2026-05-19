"""Measures repeated module import cache overhead."""

import importlib


def main() -> None:
    expected = importlib.import_module("json")
    total = 0
    for i in range(10000):
        mod = importlib.import_module("json")
        if mod is not expected:
            raise RuntimeError("import_module did not return the cached module")
        total += len(mod.__name__)
    print(total)


if __name__ == "__main__":
    main()
