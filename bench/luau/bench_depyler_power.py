"""Depyler iterative power benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/control_flow.py
Measures: tight multiplication loops (iterative exponentiation).
"""


def power_iterative(base: int, exponent: int) -> int:
    result: int = 1
    i: int = 0
    while i < exponent:
        result = result * base
        i = i + 1
    return result


def main() -> None:
    total: int = 0
    iterations: int = 0
    while iterations < 5000:
        b: int = 2
        while b <= 10:
            e: int = 1
            while e <= 15:
                total = total + power_iterative(b, e)
                e = e + 1
            b = b + 1
        iterations = iterations + 1
    print(total)


main()
