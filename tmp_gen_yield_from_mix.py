def gen():
    yield 0
    yield from [1, 2]
    yield 3


print(list(gen()))
