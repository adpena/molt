"""Purpose: differential coverage for generator decorators."""


def wrap(fn):
    def inner(n):
        for item in fn(n):
            yield item * 2

    return inner


def pack(fn):
    def inner(n):
        return list(fn(n))

    return inner


@wrap
def gen(n):
    for i in range(n):
        yield i


@pack
@wrap
def gen2(n):
    for i in range(n):
        yield i


print(list(gen(3)))
print(gen2(3))
