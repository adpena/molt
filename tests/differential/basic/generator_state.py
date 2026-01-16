import inspect


def make_gen():
    def gen():
        yield inspect.getgeneratorstate(g)

    g = gen()
    return g


g = make_gen()
print(inspect.getgeneratorstate(g))
print(next(g))
print(inspect.getgeneratorstate(g))
try:
    next(g)
except StopIteration:
    print(inspect.getgeneratorstate(g))
