"""Stress cached dotted imports after warming the requested module."""

import importlib


def main() -> None:
    expected = importlib.import_module("importlib.util")
    total = 0
    for _ in range(1_000_000):
        module = importlib.import_module("importlib.util")
        if module is not expected:
            raise RuntimeError("import_module did not return the cached module")
        total += len(module.__name__)
    print(total)


if __name__ == "__main__":
    main()
