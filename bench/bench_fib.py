"""Benchmark: recursive fibonacci(30)."""

def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

result = fib(30)
print(result)
