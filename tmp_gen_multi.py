def gen(x):
    if x == 1:
        yield 1
        return
    if x == 2:
        yield 2
        return
    yield 3


print(list(gen(1)))
print(list(gen(2)))
print(list(gen(3)))
