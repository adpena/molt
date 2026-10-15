"""Purpose: loaded method calls of every positional arity match CPython (HF-216).

Arity 0 through 6 crosses the fixed-arity runtime entries and the spilled
argument vector, with defaults, *args and keyword-only methods, for plain
functions and for bound-method attributes.
"""


class Arity:
    def zero(self):
        return ()

    def one(self, a):
        return (a,)

    def two(self, a, b):
        return (a, b)

    def three(self, a, b, c):
        return (a, b, c)

    def four(self, a, b, c, d):
        return (a, b, c, d)

    def five(self, a, b, c, d, e):
        return (a, b, c, d, e)

    def six(self, a, b, c, d, e, f):
        return (a, b, c, d, e, f)

    def defaults(self, a, b=10, c=20):
        return (a, b, c)

    def star(self, *args):
        return args

    def keyword_only(self, a, *, k=5):
        return (a, k)


arity = Arity()
print(arity.zero(), arity.one(1), arity.two(1, 2), arity.three(1, 2, 3))
print(arity.four(1, 2, 3, 4), arity.five(1, 2, 3, 4, 5))
print(arity.six(1, 2, 3, 4, 5, 6))
print(arity.defaults(1), arity.defaults(1, 2), arity.defaults(1, 2, 3))
print(arity.star(), arity.star(1, 2, 3, 4, 5, 6, 7))
print(arity.keyword_only(1))
try:
    arity.two(1)
except TypeError as error:
    print("TypeError", error)
try:
    arity.zero(1, 2, 3, 4, 5, 6)
except TypeError as error:
    print("TypeError", error)

bound = arity.three
print(bound(7, 8, 9))


class Holder:
    pass


holder = Holder()
holder.call = arity.six
print(holder.call(1, 2, 3, 4, 5, 6))


def hot():
    total = 0
    for index in range(50):
        total += sum(arity.six(index, 1, 2, 3, 4, 5))
        total += sum(arity.two(index, index))
    return total


print(hot())
