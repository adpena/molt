from molt.stdlib import collections


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
