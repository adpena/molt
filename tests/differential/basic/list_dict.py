"""Purpose: differential coverage for list dict."""

lst = [1, 2, 3]
print(lst[0])
lst[1] = 5
print(lst)
print(lst[-1])
print(lst[1:3])

d = {"a": 1, "b": 2}
print(d["a"])
d["c"] = 3
print(d)

print(lst.append(4))
print(lst)
print(lst.pop())
print(lst)
print(lst.pop(0))
print(lst)
print(lst.pop(-1))
print(lst)
print(lst.count(5))
print(lst.index(5))
print(lst.index(5, 0, 2))
try:
    print(lst.index(5, 1, 2))
except ValueError:
    print("list-index-miss")

idxer = getattr(lst, "index")
print(idxer(5, 0, 2))
try:
    idxer(5, 1, 2)
except ValueError:
    print("list-index-miss-dyn")

lst2 = [9, 8, 7]
popper = getattr(lst2, "pop")
print(popper())
print(lst2)
print(popper(0))
print(lst2)
try:
    [].pop()
except IndexError:
    print("list-pop-empty")

lst3 = [3, 1, 2]
lst3.sort()
print(lst3)


def key_first(item):
    return item[0]


lst4 = [("b", 2), ("a", 1), ("c", 0)]
lst4.sort(key=key_first)
print(lst4)
lst4.sort(key=key_first, reverse=True)
print(lst4)

lst5 = [1, 2, 3]
lst5.extend(range(4, 6))
print(lst5)
lst5.insert(-1, 9)
print(lst5)
lst5.insert(100, 7)
print(lst5)
lst5.insert(-100, 6)
print(lst5)
lst5.remove(9)
print(lst5)
try:
    lst5.remove(42)
except ValueError:
    print("list-remove-miss")
lst6 = lst5.copy()
lst5.clear()
print(lst5, lst6)
lst6.reverse()
print(lst6)

print(d.get("b"))
print(d.get("missing"))
print(d.get("missing", 9))
print(len(d.keys()))
print(len(d.values()))

big = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]
big.append(20)
print(len(big))

print(dict())
print(dict(a=1, b=2))
print(dict({"a": 1}, b=3))
print(dict(**{"x": 4, "y": 5}))
try:
    dict(**{1: 2})
except TypeError:
    print("dict-kw-typeerror")


class Mapping:
    def __init__(self):
        self.data = {"a": 1, "b": 2}

    def keys(self):
        return ["a", "b"]

    def __getitem__(self, key):
        return self.data[key]


print(dict(Mapping()))
d2 = {}
d2.update(Mapping())
print(d2["a"], d2["b"])

d3 = {"x": 1, "y": 2}
print(d3.setdefault("x", 9))
print(d3.setdefault("z", 9))
print(d3)
print(d3.pop("x"))
print(d3.pop("missing", 5))
print(dict.fromkeys(["a", "b"], 3))
print(d3.copy())
print(d3.popitem())
print(d3)
d3.clear()
print(d3)

d4 = {}
print(d4.update(a=1, b=2))
print(sorted(d4.items()))
d4 = {}
d4.update({"a": 1}, b=2)
print(sorted(d4.items()))
d4 = {}
d4.update([("a", 1)], b=2)
print(sorted(d4.items()))
d4 = {}
d4.update(**{"a": 1, "b": 2})
print(sorted(d4.items()))
try:
    d4.update(1, 2)
except TypeError as exc:
    print("dict-update-args", type(exc).__name__, exc)

d5 = {}
print(d5.__setitem__("k", 9))
print(d5.__getitem__("k"))
print(d5.__delitem__("k"))
try:
    d5.__getitem__("missing")
except KeyError as exc:
    print("dict-get-miss", type(exc).__name__, exc)


class DictSubclass(dict):
    pass


ds = DictSubclass.fromkeys(["a"], 1)
print(isinstance(ds, DictSubclass))
print(ds["a"])
print(len(ds))


class DictSubclassInit(dict):
    def __init__(self, value):
        super().__init__()


try:
    DictSubclassInit.fromkeys(["a"], 1)
except TypeError as exc:
    print("dict-fromkeys-init", type(exc).__name__)
