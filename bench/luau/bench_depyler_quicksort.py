"""Depyler quicksort benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/examples/algorithms/quicksort.py
Measures: recursive partitioning, array element swaps, divide-and-conquer.
Uses in-place quicksort variant (Luau-compatible, no list comprehensions).
"""

def partition(arr: list[int], low: int, high: int) -> int:
    pivot: int = arr[high]
    i: int = low - 1
    j: int = low
    while j < high:
        if arr[j] <= pivot:
            i = i + 1
            temp: int = arr[i]
            arr[i] = arr[j]
            arr[j] = temp
        j = j + 1
    temp2: int = arr[i + 1]
    arr[i + 1] = arr[high]
    arr[high] = temp2
    return i + 1


def quicksort(arr: list[int], low: int, high: int) -> None:
    if low < high:
        pi: int = partition(arr, low, high)
        quicksort(arr, low, pi - 1)
        quicksort(arr, pi + 1, high)


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
    while iterations < 100:
        arr: list[int] = []
        size: int = 2000
        val: int = size
        while val > 0:
            arr.append((val * 7 + 13) % size)
            val = val - 1
        quicksort(arr, 0, len(arr) - 1)
        total_sorted = total_sorted + is_sorted(arr)
        iterations = iterations + 1
    print(total_sorted)
    print(arr[0])
    print(arr[len(arr) - 1])


main()
