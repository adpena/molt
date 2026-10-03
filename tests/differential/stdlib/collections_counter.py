"""Purpose: differential coverage for collections counter."""

import collections


def show(label, value):
    print(label, value)


c = collections.Counter("abbbb")
show("keys", list(c.keys()))
show("values", list(c.values()))
show("items", list(c.items()))
show("repr", repr(c))
show("total", c.total())
show("most1", c.most_common(1))
show("most0", c.most_common(0))
show("mostneg", c.most_common(-2))
show("elements", list(c.elements()))
show("eq_counter", c == collections.Counter({"a": 1, "b": 4}))
show("eq_dict", c == {"a": 1, "b": 4})
show("eq_empty", c == collections.Counter())

c2 = collections.Counter("bcc")
show("add", c + c2)
show("sub", c - c2)
show("or", c | c2)
show("and", c & c2)

c_unary = collections.Counter({"a": 2, "b": -1, "c": 0})
show("pos", +c_unary)
show("neg", -c_unary)

c3 = collections.Counter("abbb")
c3 += collections.Counter("bcc")
show("iadd", c3)

c4 = collections.Counter("abbb")
c4 -= collections.Counter("bcc")
show("isub", c4)

c5 = collections.Counter("abbb")
c5 |= collections.Counter("bcc")
show("ior", c5)

c6 = collections.Counter("abbb")
c6 &= collections.Counter("bcc")
show("iand", c6)

c7 = collections.Counter("ab")
show("pop", c7.pop("a"))
show("pop_default", c7.pop("missing", 0))
try:
    c7.pop("missing2")
except KeyError:
    print("pop-keyerror")

c8 = collections.Counter()
show("setdefault", c8.setdefault("x", 4))
show("setdefault_again", c8.setdefault("x", 9))
show("setdefault_val", c8["x"])

c9 = collections.Counter("ab")
show("popitem", c9.popitem())
c9.clear()
show("clear", c9)

c10 = collections.Counter()
c10["bad"] = 1.5
try:
    list(c10.elements())
except TypeError:
    print("elements-typeerror")


# One physical dict storage, including explicit base descriptors and live views.
c = collections.Counter(a=2, b=3, c=4)
view = c.items()
show("dict-subclass", isinstance(c, dict))
dict.__setitem__(c, "base", 9)
show("base-storage", (c["base"], dict.__getitem__(c, "base")))
del c["b"]
c["d"] = 5
show("live-view-order", list(view))
del c["absent"]
show("missing-does-not-insert", (c["absent"], c.get("absent"), "absent" in c))
show("zero-equality", collections.Counter(a=1) == collections.Counter(a=1, z=0))
show("zero-ordering", (collections.Counter(a=1) <= collections.Counter(a=1, z=0), collections.Counter(a=1) < collections.Counter(a=2)))

# Counts use ordinary arbitrary precision and numeric protocol, without i64 casts.
c = collections.Counter(a=2**90, b=1.5, c=-2.5)
c.update({"a": 2**90, "b": 0.25})
c.subtract({"b": 0.5, "c": 0.5})
show("numeric-counts", list(c.items()))
show("numeric-total", c.total())
show("positive-counts", list((+c).items()))
show("negative-counts", list((-c).items()))
show("counter-tuple-key", list(collections.Counter([("a", 2), ("a", 2)]).items()))
show("most-common-ties", collections.Counter(c=2, b=2, a=2).most_common(2))

events = []


class Key:
    def __init__(self, name):
        self.name = name

    def __hash__(self):
        events.append("hash:" + self.name)
        return 7

    def __eq__(self, other):
        events.append("eq:" + self.name + ":" + other.name)
        return self.name == other.name


key = Key("one")
c = collections.Counter([key, key])
show("tally-hash-calls", events)
show("tally-value", list(c.values()))


class DerivedCounter(collections.Counter):
    def __missing__(self, key):
        return 17

    def get(self, key, default=None):
        events.append("get:" + str(key))
        return super().get(key, default)

    def __setitem__(self, key, value):
        events.append("set:" + str(key))
        super().__setitem__(key, value)


events.clear()
c = DerivedCounter(["x", "x"])
show("subclass-tally", (events, list(c.items()), c["missing"]))
show("subclass-copy", type(c.copy()).__name__)


class StringKey(str):
    def __hash__(self):
        return str.__hash__(self)

    def __eq__(self, other):
        events.append("string-eq")
        return False


events.clear()
c = collections.Counter()
c[StringKey("same")] = 1
c["same"] = 2
show("string-subclass", (len(c), list(c.values()), events))


class HashFailure:
    def __hash__(self):
        raise ValueError("key-hash")


c = collections.Counter()
try:
    c.update(iter(["before", HashFailure(), "after"]))
except ValueError as error:
    show("update-error", str(error))
show("update-prefix", list(c.items()))


class ComparisonFailure:
    def __eq__(self, other):
        raise ValueError("count-equality")


try:
    collections.Counter(x=ComparisonFailure()) == collections.Counter(x=0)
except ValueError as error:
    show("count-equality-error", str(error))


class ReentrantKey:
    def __hash__(self):
        return 12

    def __eq__(self, other):
        c["callback"] = 8
        return False


c = collections.Counter()
c[ReentrantKey()] = 1
c[ReentrantKey()] = 2
show("callback-reentry", (c["callback"], list(c.values())))

c = collections.Counter(a=1, b=2)
iterator = iter(c)
show("iteration-first", next(iterator))
c["c"] = 3
try:
    next(iterator)
except RuntimeError as error:
    show("iteration-mutation", type(error).__name__)

c = collections.Counter(a=2, b=1)
elements = c.elements()
c["a"] = 3
show("elements-live-value", list(elements))

c = collections.Counter(a=1, b=2)
alias = c
try:
    c += {"a": 3, "b": "invalid"}
except TypeError:
    show("inplace-error-prefix", (c is alias, list(c.items())))

try:
    collections.Counter.fromkeys("abc")
except NotImplementedError:
    show("fromkeys", "undefined")

show("keyword-iterable", list(collections.Counter(iterable=4).items()))
