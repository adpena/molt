"""Depyler compute-intensive benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/benchmarks/python/compute_intensive.py
Measures: integer arithmetic, iterative fibonacci, list accumulation, statistics.
Depyler claims 12.36x speedup over CPython for this workload.
"""

def fibonacci_iterative(n: int) -> int:
    if n <= 1:
        return n
    a: int = 0
    b: int = 1
    i: int = 2
    while i <= n:
        c: int = a + b
        a = b
        b = c
        i = i + 1
    return b


def sum_fibonacci_numbers(limit: int) -> int:
    total: int = 0
    i: int = 0
    while i < limit:
        total = total + fibonacci_iterative(i)
        i = i + 1
    return total


def calculate_statistics(numbers: list[int]) -> list[int]:
    count: int = len(numbers)
    if count == 0:
        return [0, 0, 0, 0]
    total: int = 0
    min_val: int = numbers[0]
    max_val: int = numbers[0]
    i: int = 0
    while i < count:
        num: int = numbers[i]
        total = total + num
        if num < min_val:
            min_val = num
        if num > max_val:
            max_val = num
        i = i + 1
    return [count, total, min_val, max_val]


def main() -> None:
    limits: list[int] = [25, 30, 35, 38, 40]
    runs: int = 0
    last_result: int = 0
    last_max: int = 0
    while runs < 5000:
        idx: int = 0
        while idx < 5:
            limit: int = limits[idx]
            result: int = sum_fibonacci_numbers(limit)
            fib_sequence: list[int] = []
            i: int = 0
            while i < limit:
                fib_sequence.append(fibonacci_iterative(i))
                i = i + 1
            stats: list[int] = calculate_statistics(fib_sequence)
            last_result = result
            last_max = stats[3]
            idx = idx + 1
        runs = runs + 1
    print(last_result)
    print(last_max)


main()
