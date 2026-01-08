def foo(x: int) -> int:
    return x + 1


def bar(x: int) -> int:
    return x + 2


print(foo(1))
foo = bar
print(foo(1))
