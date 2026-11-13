"""Parallel assignment ``a, b = x, y``: every value left to right, then every
target left to right, with no tuple observable, against CPython."""


class Log:
    def __init__(self, name):
        self.name = name
        self.items = {}

    def __getitem__(self, key):
        print("get", self.name, key)
        return self.items.get(key, 0)

    def __setitem__(self, key, value):
        print("set", self.name, key, value)
        self.items[key] = value


def value(label, result):
    print("value", label)
    return result


def key(label, result):
    print("key", label)
    return result


a, b = 1, 2
a, b = b, a
print("swap", a, b)

x = [10, 20, 30]
x[0], x[1] = x[1], x[0]
print("list swap", x)

# The second target reads the index the first target just rebound.
i = 0
x = [10, 20, 30]
i, x[i] = 2, 99
print("rebind then index", i, x)
i = 0
x = [10, 20, 30]
x[i], i = 99, 2
print("index then rebind", i, x)

log = Log("L")
log[key("t1", 1)], log[key("t2", 2)] = value("v1", 5), value("v2", 6)
print("items", sorted(log.items.items()))

a, b, c = value("a", 1), value("b", 2), value("c", 3)
print("three", a, b, c)
[p, q] = value("p", "P"), value("q", "Q")
print("list target", p, q)
(r, s) = [value("r", 1), value("s", 2)]
print("list value", r, s)
(t, (u, v)) = value("t", 1), (value("u", 2), value("v", 3))
print("nested", t, u, v)
first, *rest = 1, 2, 3
print("starred", first, rest)


def fails():
    raise ValueError("value two")


try:
    a, b = value("one", 1), fails()
except ValueError as exc:
    print("raised", exc, a, b)

try:
    a, b = 1, 2, 3
except ValueError as exc:
    print("mismatch", exc)


def in_function(seq):
    seq[0], seq[-1] = seq[-1], seq[0]
    lo, hi = 0, len(seq) - 1
    while lo < hi:
        seq[lo], seq[hi] = seq[hi], seq[lo]
        lo, hi = lo + 1, hi - 1
    return seq


print("reverse", in_function(list(range(7))))


def generator():
    a, b = (yield 1), (yield 2)
    yield (a, b)


g = generator()
print("gen", next(g), g.send("A"), g.send("B"))
