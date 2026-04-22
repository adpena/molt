"""Depyler string operations benchmark.

Ported from: https://github.com/paiml/depyler/blob/main/tests/fixtures/python_samples/string_operations.py
Measures: string building, character iteration, substring search.
"""


def repeat_string(s: str, n: int) -> str:
    result: str = ""
    i: int = 0
    while i < n:
        result = result + s
        i = i + 1
    return result


def count_char(s: str, c: str) -> int:
    count: int = 0
    i: int = 0
    while i < len(s):
        if s[i] == c:
            count = count + 1
        i = i + 1
    return count


def reverse_string(s: str) -> str:
    result: str = ""
    i: int = len(s) - 1
    while i >= 0:
        result = result + s[i]
        i = i - 1
    return result


def main() -> None:
    iterations: int = 0
    total: int = 0
    while iterations < 2000:
        s: str = repeat_string("abcdefghij", 100)
        total = total + len(s)
        total = total + count_char(s, "a")
        rev: str = reverse_string(s)
        total = total + len(rev)
        iterations = iterations + 1
    print(total)


main()
