def fib_recursive(n: int) -> int:
    if n <= 1:
        return n
    return fib_recursive(n - 1) + fib_recursive(n - 2)


def fib_iterative(n: int) -> int:
    a: int = 0
    b: int = 1
    i: int = 0
    while i < n:
        temp: int = b
        b = a + b
        a = temp
        i = i + 1
    return a


def main() -> None:
    result: int = fib_recursive(30)
    print(result)
    result2: int = fib_iterative(1000)
    print(result2)


main()
