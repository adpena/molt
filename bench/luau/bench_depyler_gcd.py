"""Depyler GCD benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/control_flow.py
Measures: while-loop throughput, modulo arithmetic (Euclidean algorithm).
"""

def gcd(a: int, b: int) -> int:
    while b != 0:
        temp: int = b
        b = a % b
        a = temp
    return a


def main() -> None:
    total: int = 0
    iterations: int = 0
    while iterations < 50000:
        a: int = 1
        while a <= 500:
            b: int = 1
            while b <= 500:
                total = total + gcd(a, b)
                b = b + 50
            a = a + 50
        iterations = iterations + 1
    print(total)


main()
