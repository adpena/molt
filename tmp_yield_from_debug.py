def gen():
    yield from [1, 2, 3]


print(list(gen()))
