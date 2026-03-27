"""Purpose: builtin default arguments should preserve captured builtin callables."""


def call_default(value, marker=None, fn=str):
    print(marker is None)
    return fn(value)


print(call_default(123))
