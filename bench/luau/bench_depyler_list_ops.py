"""Depyler list operations benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/list_operations.py
Measures: list append, iteration, element access, filtering.
"""


def sum_list(numbers: list[int]) -> int:
    total: int = 0
    i: int = 0
    while i < len(numbers):
        total = total + numbers[i]
        i = i + 1
    return total


def find_max(numbers: list[int]) -> int:
    if len(numbers) == 0:
        return 0
    max_val: int = numbers[0]
    i: int = 1
    while i < len(numbers):
        if numbers[i] > max_val:
            max_val = numbers[i]
        i = i + 1
    return max_val


def filter_positive(numbers: list[int]) -> list[int]:
    result: list[int] = []
    i: int = 0
    while i < len(numbers):
        if numbers[i] > 0:
            result.append(numbers[i])
        i = i + 1
    return result


def reverse_list(numbers: list[int]) -> list[int]:
    result: list[int] = []
    i: int = len(numbers) - 1
    while i >= 0:
        result.append(numbers[i])
        i = i - 1
    return result


def main() -> None:
    iterations: int = 0
    total: int = 0
    while iterations < 500:
        arr: list[int] = []
        i: int = 0
        while i < 2000:
            arr.append(i - 1000)
            i = i + 1

        total = total + sum_list(arr)
        total = total + find_max(arr)

        positive: list[int] = filter_positive(arr)
        total = total + len(positive)

        rev: list[int] = reverse_list(arr)
        total = total + rev[0]

        iterations = iterations + 1
    print(total)


main()
