"""Depyler digit-manipulation benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/control_flow.py
Measures: integer modulo/division throughput (sum_of_digits, reverse_integer).
"""

def sum_of_digits(n: int) -> int:
    total: int = 0
    while n > 0:
        total = total + n % 10
        n = n // 10
    return total


def reverse_integer(n: int) -> int:
    result: int = 0
    while n > 0:
        result = result * 10 + n % 10
        n = n // 10
    return result


def main() -> None:
    total: int = 0
    iterations: int = 0
    while iterations < 200:
        n: int = 1
        while n <= 10000:
            total = total + sum_of_digits(n)
            total = total + reverse_integer(n)
            n = n + 1
        iterations = iterations + 1
    print(total)


main()
