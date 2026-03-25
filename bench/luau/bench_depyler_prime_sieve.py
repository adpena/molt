"""Depyler prime check benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/control_flow.py
Measures: integer arithmetic, modulo, sqrt approximation, tight loops.
Uses Depyler's is_prime plus a counting loop to create meaningful work.
"""

def is_prime(n: int) -> int:
    if n < 2:
        return 0
    if n < 4:
        return 1
    if n % 2 == 0:
        return 0
    d: int = 3
    while d * d <= n:
        if n % d == 0:
            return 0
        d = d + 2
    return 1


def count_primes(limit: int) -> int:
    count: int = 0
    n: int = 2
    while n <= limit:
        count = count + is_prime(n)
        n = n + 1
    return count


def main() -> None:
    result: int = count_primes(500000)
    print(result)


main()
