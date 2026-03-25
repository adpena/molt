"""Depyler bubble sort benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/control_flow.py
Measures: nested loops, array element swaps, comparison-heavy workload.
"""

def bubble_sort(arr: list[int]) -> list[int]:
    n: int = len(arr)
    i: int = 0
    while i < n:
        j: int = 0
        while j < n - i - 1:
            if arr[j] > arr[j + 1]:
                temp: int = arr[j]
                arr[j] = arr[j + 1]
                arr[j + 1] = temp
            j = j + 1
        i = i + 1
    return arr


def is_sorted(arr: list[int]) -> int:
    i: int = 0
    while i < len(arr) - 1:
        if arr[i] > arr[i + 1]:
            return 0
        i = i + 1
    return 1


def main() -> None:
    iterations: int = 0
    total_sorted: int = 0
    while iterations < 50:
        arr: list[int] = []
        size: int = 500
        val: int = size
        while val > 0:
            arr.append(val)
            val = val - 1
        sorted_arr: list[int] = bubble_sort(arr)
        total_sorted = total_sorted + is_sorted(sorted_arr)
        iterations = iterations + 1
    print(total_sorted)
    print(sorted_arr[0])
    print(sorted_arr[len(sorted_arr) - 1])


main()
