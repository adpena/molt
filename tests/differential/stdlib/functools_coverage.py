import functools

def add(a, b):
    return a + b
add5 = functools.partial(add, 5)
print(add5(3))

result = functools.reduce(lambda a, b: a + b, [1, 2, 3, 4])
print(result)

@functools.lru_cache(maxsize=32)
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)
print(fib(10))
