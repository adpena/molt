def gen(n):
    if n == 0:
        yield 0
        return
    for x in gen(n - 1):
        yield x + 1


print(list(gen(1)))
