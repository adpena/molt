"""Purpose: differential coverage for collections module — namedtuple, OrderedDict, defaultdict, Counter, deque."""

from collections import Counter, OrderedDict, defaultdict, deque, namedtuple

# ========== namedtuple ==========

Point = namedtuple("Point", ["x", "y"])

p = Point(1, 2)
print("nt repr", p)
print("nt x", p.x)
print("nt y", p.y)
print("nt index", p[0], p[1])
print("nt unpack", *p)
print("nt eq", p == Point(1, 2))
print("nt neq", p == Point(2, 1))
print("nt isinstance", isinstance(p, tuple))

# _asdict
d = p._asdict()
print("nt asdict", d)
print("nt asdict type", type(d).__name__)

# _replace
p2 = p._replace(x=10)
print("nt replace", p2)
print("nt replace original", p)

# _make
p3 = Point._make([3, 4])
print("nt make", p3)

# _fields
print("nt fields", Point._fields)

# _field_defaults
PointDefault = namedtuple("PointDefault", ["x", "y", "z"], defaults=[0, 0])
pd = PointDefault(1)
print("nt defaults", pd)
print("nt field_defaults", PointDefault._field_defaults)

# namedtuple with rename
NT = namedtuple("NT", ["class", "x", "def", "y"], rename=True)
print("nt rename fields", NT._fields)

# namedtuple from string field names
Color = namedtuple("Color", "r g b")
c = Color(255, 128, 0)
print("nt string fields", c)

# Hashing
print("nt hash eq", hash(Point(1, 2)) == hash(Point(1, 2)))
print("nt hash as key", {Point(1, 2): "origin"}[Point(1, 2)])

# Comparison inherits from tuple
print("nt lt", Point(1, 2) < Point(2, 0))
print("nt le", Point(1, 2) <= Point(1, 2))

# ========== OrderedDict ==========

od = OrderedDict()
od["a"] = 1
od["b"] = 2
od["c"] = 3
print("od keys", list(od.keys()))
print("od values", list(od.values()))

# Insertion order preserved
od["a"] = 10  # update existing — keeps position
print("od update keys", list(od.keys()))

# move_to_end
od.move_to_end("a")
print("od move_to_end", list(od.keys()))

od.move_to_end("c", last=False)
print("od move_to_end first", list(od.keys()))

# Equality: OrderedDict vs OrderedDict respects order
od1 = OrderedDict([("a", 1), ("b", 2)])
od2 = OrderedDict([("b", 2), ("a", 1)])
print("od eq order matters", od1 == od2)

# OrderedDict vs dict: order does NOT matter
d1 = {"a": 1, "b": 2}
print("od eq dict", od1 == d1)

# popitem LIFO (default)
od3 = OrderedDict([("x", 1), ("y", 2), ("z", 3)])
print("od popitem", od3.popitem())
print("od popitem first", od3.popitem(last=False))
print("od after popitem", list(od3.keys()))

# fromkeys
od4 = OrderedDict.fromkeys(["p", "q", "r"], 0)
print("od fromkeys", list(od4.items()))

# ========== defaultdict ==========

# int factory
dd_int = defaultdict(int)
dd_int["a"] += 1
dd_int["a"] += 1
dd_int["b"] += 1
print("dd int", dict(dd_int))

# list factory
dd_list = defaultdict(list)
dd_list["x"].append(1)
dd_list["x"].append(2)
dd_list["y"].append(3)
print("dd list", dict(dd_list))

# set factory
dd_set = defaultdict(set)
dd_set["s"].add(1)
dd_set["s"].add(2)
dd_set["s"].add(1)
print("dd set", {k: sorted(v) for k, v in dd_set.items()})

# lambda factory
dd_lam = defaultdict(lambda: "missing")
dd_lam["a"] = "found"
print("dd lambda", dd_lam["a"], dd_lam["b"])

# None factory (KeyError)
dd_none = defaultdict()
try:
    _ = dd_none["missing"]
    print("dd none", "no error")
except KeyError:
    print("dd none", "KeyError")

# default_factory attribute
print("dd factory attr", dd_int.default_factory)
dd_int.default_factory = float
dd_int["c"]
print("dd changed factory", dd_int["c"])

# ========== Counter ==========

cnt = Counter("abracadabra")
print("cnt a", cnt["a"])
print("cnt b", cnt["b"])
print("cnt z", cnt["z"])  # missing key returns 0

# most_common
print("cnt most_common 3", cnt.most_common(3))

# elements
print("cnt elements sorted", sorted(cnt.elements()))

# Arithmetic
c1 = Counter(a=3, b=1)
c2 = Counter(a=1, b=2)
print("cnt add", dict(c1 + c2))
print("cnt sub", dict(c1 - c2))
print("cnt intersect", dict(c1 & c2))
print("cnt union", dict(c1 | c2))

# Unary +/- (remove zero/negative)
c3 = Counter(a=2, b=-1, c=0)
print("cnt unary plus", dict(+c3))
print("cnt unary minus", dict(-c3))

# update and subtract
c4 = Counter(a=1, b=2)
c4.update({"a": 3, "c": 1})
print("cnt update", dict(c4))

c4.subtract({"a": 2, "b": 1})
print("cnt subtract", dict(c4))

# total
c5 = Counter(a=10, b=20)
print("cnt total", c5.total())

# Construction from various inputs
print("cnt from list", dict(Counter([1, 1, 2, 3, 3, 3])))
print("cnt from dict", dict(Counter({"x": 5, "y": 3})))
print("cnt from kwargs", dict(Counter(red=4, blue=2)))

# Equality
print("cnt eq", Counter("abc") == Counter("bca"))
print("cnt neq", Counter("abc") == Counter("abcc"))

# ========== deque ==========

dq = deque([1, 2, 3])
print("dq repr", dq)
print("dq len", len(dq))
print("dq index", dq[0], dq[-1])

# appendleft / popleft
dq.appendleft(0)
print("dq appendleft", list(dq))

left = dq.popleft()
print("dq popleft", left, list(dq))

# append / pop
dq.append(4)
print("dq append", list(dq))

right = dq.pop()
print("dq pop", right, list(dq))

# extend / extendleft
dq.extend([4, 5])
print("dq extend", list(dq))

dq.extendleft([0, -1])  # note: added in reverse order
print("dq extendleft", list(dq))

# rotate
dq2 = deque([1, 2, 3, 4, 5])
dq2.rotate(2)
print("dq rotate right", list(dq2))

dq2.rotate(-2)
print("dq rotate back", list(dq2))

dq2.rotate(-1)
print("dq rotate left", list(dq2))

# maxlen
dq_max = deque([1, 2, 3], maxlen=4)
dq_max.append(4)
print("dq maxlen append", list(dq_max))

dq_max.append(5)  # drops left
print("dq maxlen overflow", list(dq_max))

dq_max.appendleft(0)  # drops right
print("dq maxlen overflow left", list(dq_max))

print("dq maxlen attr", dq_max.maxlen)

# count and index
dq3 = deque([1, 2, 3, 2, 1])
print("dq count", dq3.count(2))
print("dq index", dq3.index(3))

# reverse
dq4 = deque([1, 2, 3])
dq4.reverse()
print("dq reverse", list(dq4))

# copy
dq5 = deque([1, 2, 3])
dq6 = dq5.copy()
dq5.append(4)
print("dq copy isolation", list(dq5), list(dq6))

# remove
dq7 = deque([1, 2, 3, 2])
dq7.remove(2)  # removes first occurrence
print("dq remove", list(dq7))

# clear
dq7.clear()
print("dq clear", list(dq7))

# Comparison
print("dq eq", deque([1, 2]) == deque([1, 2]))
print("dq neq", deque([1, 2]) == deque([2, 1]))
print("dq lt", deque([1, 2]) < deque([1, 3]))

# Boolean
print("dq bool empty", bool(deque()))
print("dq bool nonempty", bool(deque([1])))

# Iteration
dq8 = deque([10, 20, 30])
print("dq iter", [x for x in dq8])
print("dq reversed", [x for x in reversed(dq8)])

# in operator
print("dq contains", 20 in dq8)
print("dq not contains", 99 in dq8)

# deque from empty
dq_empty = deque()
print("dq empty", len(dq_empty), list(dq_empty))
dq_empty.append(1)
print("dq empty after append", list(dq_empty))
