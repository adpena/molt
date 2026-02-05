"""Purpose: ensure recursive generator calls allocate correct task closure size."""


def gen(n):
    if n <= 0:
        return
    yield n
    for val in gen(n - 1):
        yield val


print(list(gen(4)))
