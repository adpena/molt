"""Depyler binary search benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/control_flow.py
and https://github.com/paiml/depyler/blob/main/examples/algorithms/binary_search_simple.py
Measures: loop-based searching, integer comparison, array access.
"""


def binary_search(arr: list[int], target: int) -> int:
    left: int = 0
    right: int = len(arr) - 1
    while left <= right:
        mid: int = (left + right) // 2
        if arr[mid] == target:
            return mid
        elif arr[mid] < target:
            left = mid + 1
        else:
            right = mid - 1
    return -1


def linear_search(arr: list[int], target: int) -> int:
    i: int = 0
    while i < len(arr):
        if arr[i] == target:
            return i
        i = i + 1
    return -1


def main() -> None:
    size: int = 10000
    arr: list[int] = []
    i: int = 0
    while i < size:
        arr.append(i * 2)
        i = i + 1

    found_bin: int = 0
    found_lin: int = 0
    iterations: int = 0
    while iterations < 500:
        target: int = 0
        while target < size:
            idx: int = binary_search(arr, target * 2)
            if idx >= 0:
                found_bin = found_bin + 1
            target = target + 50
        iterations = iterations + 1

    iterations = 0
    while iterations < 10:
        target2: int = 0
        while target2 < 1000:
            idx2: int = linear_search(arr, target2 * 2)
            if idx2 >= 0:
                found_lin = found_lin + 1
            target2 = target2 + 50
        iterations = iterations + 1

    print(found_bin)
    print(found_lin)


main()
