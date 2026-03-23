"""Depyler factorial benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/control_flow.py
Measures: recursive function calls, integer multiplication.
"""

def factorial(n: int) -> int:
    if n <= 1:
        return 1
    return n * factorial(n - 1)


def main() -> None:
    total: int = 0
    run: int = 0
    while run < 50000:
        i: int = 1
        while i <= 20:
            total = total + factorial(i)
            i = i + 1
        run = run + 1
    print(total)


main()
