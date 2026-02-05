def gen(use=False):
    if use:
        yield from [2]
        return
    yield 1


print(list(gen(False)))
print(list(gen(True)))
