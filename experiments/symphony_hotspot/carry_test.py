"""Minimal test for loop carry var optimization."""


def bench() -> None:
    total: int = 0
    i: int = 0
    while i < 10:
        total = total + 1
        i = i + 1
    print("Result: " + str(total))


bench()
